#!/usr/bin/env bash
# cleanup-pipewire-loopback.sh
#
# Removes PipeWire module-loopback instances that route your microphone to your
# speakers. These persist until logout/reboot even after Flint exits.
#
# This can happen if you previously ran the old (incorrect) Flint health-check
# advice: `pactl load-module module-loopback latency_msec=1`
#
# Safe to run anytime. Idempotent — does nothing if no loopback modules exist.

set -euo pipefail

if ! command -v pactl &>/dev/null; then
  echo "Error: pactl not found. Install pipewire-pulse or pulseaudio-utils." >&2
  exit 1
fi

mapfile -t MODULE_IDS < <(pactl list modules short 2>/dev/null | awk '/module-loopback/ {print $1}')

if [[ ${#MODULE_IDS[@]} -eq 0 ]]; then
  echo "No module-loopback instances loaded. Your audio routing is clean."
  exit 0
fi

echo "Unloading ${#MODULE_IDS[@]} module-loopback instance(s)..."
for id in "${MODULE_IDS[@]}"; do
  pactl unload-module "$id"
  echo "  unloaded module $id"
done

echo ""
echo "Done. Mic should no longer play through your speakers."
echo "If echo persists, log out and back in (or reboot) to reset PipeWire."
