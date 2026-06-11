#!/usr/bin/env bash
# install-whisper-model.sh
#
# Downloads a Whisper.cpp GGML model into ~/.cache/whisper/.
#
# Usage:
#   ./scripts/install-whisper-model.sh [model]
#
# Where [model] is one of: tiny.en  base.en  small.en  medium.en  large-v3
# Defaults to small.en (best accuracy/speed tradeoff for Flint on tier 3/4 hardware).
#
# Flint loads the first model that exists in this priority order:
#   medium.en > small.en > base.en > tiny.en
# So installing small.en is enough to immediately switch away from base.en.
#
# Examples:
#   ./scripts/install-whisper-model.sh           # installs small.en
#   ./scripts/install-whisper-model.sh medium.en # installs medium.en (~770 MB)

set -euo pipefail

MODEL="${1:-small.en}"

HF_BASE="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

declare -A MODEL_FILES=(
  ["tiny.en"]="ggml-tiny.en.bin"
  ["base.en"]="ggml-base.en.bin"
  ["small.en"]="ggml-small.en.bin"
  ["medium.en"]="ggml-medium.en.bin"
  ["large-v3"]="ggml-large-v3.bin"
)

# Minimum expected sizes in bytes — guards against silent partial downloads.
declare -A MODEL_MIN_BYTES=(
  ["tiny.en"]="70000000"
  ["base.en"]="140000000"
  ["small.en"]="240000000"
  ["medium.en"]="760000000"
  ["large-v3"]="3000000000"
)

if [[ -z "${MODEL_FILES[$MODEL]+_}" ]]; then
  echo "Unknown model: $MODEL" >&2
  echo "Valid options: ${!MODEL_FILES[*]}" >&2
  exit 1
fi

FILENAME="${MODEL_FILES[$MODEL]}"
DEST_DIR="$HOME/.cache/whisper"
DEST="$DEST_DIR/$FILENAME"

mkdir -p "$DEST_DIR"

if [[ -f "$DEST" ]]; then
  SIZE=$(du -sh "$DEST" | cut -f1)
  echo "$FILENAME already installed ($SIZE). Nothing to do."
  exit 0
fi

URL="$HF_BASE/$FILENAME"

echo "Downloading $MODEL ..."
echo "  from: $URL"
echo "  to:   $DEST"
echo ""

if command -v curl &>/dev/null; then
  curl -L --progress-bar -o "$DEST.tmp" "$URL"
elif command -v wget &>/dev/null; then
  wget --show-progress -O "$DEST.tmp" "$URL"
else
  echo "Error: neither curl nor wget found. Install one and retry." >&2
  exit 1
fi

# Verify the downloaded file meets the minimum expected size before promoting.
ACTUAL_BYTES=$(wc -c < "$DEST.tmp")
MIN_BYTES="${MODEL_MIN_BYTES[$MODEL]}"
if [[ "$ACTUAL_BYTES" -lt "$MIN_BYTES" ]]; then
  rm -f "$DEST.tmp"
  echo "" >&2
  echo "Error: downloaded file is only ${ACTUAL_BYTES} bytes (expected >= ${MIN_BYTES})." >&2
  echo "The download appears truncated. Check your network and retry." >&2
  exit 1
fi

mv "$DEST.tmp" "$DEST"

SIZE=$(du -sh "$DEST" | cut -f1)
echo ""
echo "Installed $FILENAME ($SIZE) to $DEST"
echo ""
echo "Restart Flint. It will auto-select this model (no config needed)."
