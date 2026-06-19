# M8 — Input Quality Manual QA

Run on a real device with speakers/headphones and Whisper model installed.

## Prerequisites

- Health check passes (mic + system loopback).
- `~/.cache/whisper/ggml-*.bin` present.

## Phase 1 — System audio calibration

- [ ] From session flow: Session Focus → Mic Calibration → Run system audio test.
- [ ] TTS plays reference clip; loopback capture runs ~30s.
- [ ] WER displayed; pass when WER < 20%.
- [ ] Fail path shows routing guidance (PipeWire monitor / BlackHole).

## Phase 2 — Microphone calibration

- [ ] Read displayed paragraph aloud; test completes within 45s.
- [ ] WER displayed; pass when WER < 25%.
- [ ] Completion copy mentions bottom-right quality badge during live sessions.

## Skip / re-test flows

- [ ] After pass on device, skip gate offers **Run again** and **Skip — nothing changed**.
- [ ] Settings → **Re-test mic and audio** opens calibration in force mode.

## Failure UX

- [ ] Fail shows red warning and recommendation list.
- [ ] **I understand — continue anyway** marks calibration with `forced` flag.

## Live quality badge

- [ ] During rehearsal/mock/live, poor mic logprob triggers amber **Mic quality low** badge bottom-right.
- [ ] Badge clears when quality recovers.

## Platform matrix

| Check | Linux | macOS | Windows |
|-------|-------|-------|---------|
| System loopback WER | | | |
| Mic WER | | | |
| Quality badge timing | | | |

Record results:

```
/flint-loop resume — m8-3.5.4-system-audio-test <platform> pass|fail (<notes>)
/flint-loop resume — m8-3.5.5-mic-test <platform> pass|fail (<notes>)
/flint-loop resume — m8-3.5.8-quality-badge <platform> pass|fail (<notes>)
```
