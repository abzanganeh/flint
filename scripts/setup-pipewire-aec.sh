#!/usr/bin/env bash
# setup-pipewire-aec.sh
#
# Loads PipeWire's WebRTC acoustic echo canceller (AEC) and creates a virtual
# microphone source that subtracts your speaker output from your mic signal.
#
# Run once before starting a Flint live session. The setup persists until the
# next logout/reboot. Re-running is safe (existing module is detected and skipped).
#
# After this runs, Flint will auto-detect the echo-cancel source and prefer it
# over the raw mic. You can verify with:
#   pactl list sources short | grep echo
#
# To make the AEC permanent across reboots, add the lines under "Manual" below
# to ~/.config/pipewire/pipewire.conf.d/99-echo-cancel.conf (create if absent).
#
# Requirements: pipewire-pulse or pulseaudio-utils (pactl), pipewire >= 0.3.

set -euo pipefail

SINK=$(pactl get-default-sink 2>/dev/null || true)
SOURCE=$(pactl get-default-source 2>/dev/null || true)

if [[ -z "$SINK" || -z "$SOURCE" ]]; then
  echo "Error: could not determine default sink/source. Is PipeWire/PulseAudio running?" >&2
  exit 1
fi

# Check if the module is already loaded.
if pactl list modules short 2>/dev/null | grep -q "module-echo-cancel"; then
  echo "module-echo-cancel is already loaded."
  echo ""
  echo "Active echo-cancel sources:"
  pactl list sources short | grep -i "echo\|cancel\|aec" || echo "  (none visible yet — wait a moment and re-run)"
  exit 0
fi

echo "Loading module-echo-cancel ..."
echo "  sink:   $SINK"
echo "  source: $SOURCE"
echo ""

# aec_method=webrtc is the highest quality; it also supports beamforming on
# systems with multi-mic arrays. The source_name must not contain spaces.
pactl load-module module-echo-cancel \
  source_name=echo_cancel_source \
  sink_name=echo_cancel_sink \
  source_master="$SOURCE" \
  sink_master="$SINK" \
  aec_method=webrtc \
  use_volume_sharing=yes \
  use_master_format=yes

echo ""
echo "Done. Echo-cancel virtual source created: echo_cancel_source"
echo ""
echo "Flint will now auto-detect this source when you start a live session."
echo "To verify:"
echo "  pactl list sources short | grep echo"
echo ""
echo "To remove before logout:"
echo "  pactl unload-module module-echo-cancel"
echo ""
echo "--- Manual permanent setup (runs at every login) ---"
echo "Create ~/.config/pipewire/pipewire.conf.d/99-echo-cancel.conf with:"
cat <<'EOF'
context.modules = [
  {
    name = libpipewire-module-echo-cancel
    args = {
      aec.method = webrtc
      source.props = { node.name = echo_cancel_source }
      sink.props   = { node.name = echo_cancel_sink }
    }
  }
]
EOF
