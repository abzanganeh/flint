# Stealth & audio hardware validation (manual gate)

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
| OBS / stealth on Wayland | **Accepted** | Full-monitor screencast may include Flint; content protection is best-effort |
| macOS stealth | Requires capture exclusion APIs | Verified via HealthCheck + `run_stealth_self_test` at live start |

---

## Platform matrix

Run before v1 release. Attach logs (`RUST_LOG=info`) and HealthCheck screenshot.

### Linux (Wayland)

**Prerequisites:** PipeWire, Wayland session (X11 fails stealth gate), screencast permission.

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| L1 | HealthCheck | `stealth_api`, `system_audio_loopback`, `microphone_access`, `global_hotkey`, `system_audio_isolation` all pass/warn acceptably | ☐ |
| L2 | System audio loopback | Play YouTube/browser audio — must appear on **System** channel only (not Mic). Zoom/Meet also valid (see `m13-live-pipeline-checklist.md` §A). **Do not mark PASS without device evidence.** | ☐ — retest after pulse-first fix (see root-cause section above) |
| L3 | Hotkeys **with focus** | Ctrl+Alt+Space re-ask and Ctrl+Alt+Shift+Space panic hide work while Flint/overlay focused; Linux fallbacks (Ctrl+Shift+Space, F8 dev) documented in slice 3 | ☐ — retest after slice 3 hotkey fallbacks land |
| L4 | Hotkeys **without focus** | Record pass/fail — expected fail on Wayland until portal work lands | ☐ — document outcome; accepted P2 if fail |
| L5 | OBS / screen capture | Start OBS full-display capture; note whether Flint overlay is visible (document outcome) | ☐ |

### macOS

**Prerequisites:** [BlackHole 2ch](https://existential.audio/blackhole/) + Multi-Output Device (Speakers + BlackHole).

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| M1 | HealthCheck | BlackHole detected; stealth + mic checks pass | ☐ |
| M2 | System audio via BlackHole | Interviewer audio on System channel during live session | ☐ |
| M3 | OBS capture | Overlay excluded or documented as visible (record which) | ☐ |
| M4 | Global hotkeys unfocused | Ctrl+Q works while another app is focused | ☐ |

### Windows

**Prerequisites:** Default output device; WASAPI loopback (no virtual cable required).

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| W1 | HealthCheck | System audio loopback reported as supported | ☐ |
| W2 | WASAPI loopback | Meet/Zoom/browser audio on System channel | ☐ |
| W3 | OBS capture | Stealth / capture-exclusion behavior recorded | ☐ |
| W4 | Global hotkeys unfocused | Ctrl+Q works while another app is focused | ☐ |

---

## Phone interview mode (all platforms)

Cross-reference `m13-live-pipeline-checklist.md` phone-mode scenarios and slice 9 diarization:

| # | Scenario | Pass criteria | Result |
| --- | --- | --- | --- |
| P1 | Phone mode live start | Single mic channel; banner + Ctrl+Q manual boundary works | ☐ |
| P2 | speakrs models | Download via Settings; SpeakerPicker appears when 2 speakers detected | ☐ |
| P3 | Diarization fallback | Ctrl+Q still works when models missing or diarization fails | ☐ |

---

## Recording results

For each run, save:

1. OS + version (e.g. Ubuntu 24.04 Wayland, macOS 14, Windows 11)
2. HealthCheck export or screenshot
3. Pass/fail per row above
4. Link to `~/.flint/metrics.log` session summary if live pipeline tested

File results under `tests/manual-qa/` as `stealth-audio-validation-YYYY-MM-DD.md` when complete.

---

## Manual gate closure

Mark **stealth/audio hardware validation** done in release docs only when:

- [ ] All platform rows L1–L4 / M1–M4 / W1–W4 have recorded pass/fail
- [ ] Wayland hotkey-without-focus outcome explicitly documented (pass or accepted fail)
- [ ] OBS capture outcome documented per platform
- [ ] No open **blocker** severities remain (P2/P3 may stay open with documented acceptance)

Until then, leave in `manual_gate_backlog`.
