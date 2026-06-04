#!/usr/bin/env bash
set -euo pipefail

# Tauri 2.x Linux build dependencies for Flint (Ubuntu/Debian).
# Run once on a fresh dev machine: ./scripts/install-linux-deps.sh

if ! command -v apt-get >/dev/null 2>&1; then
  echo "This script targets Debian/Ubuntu (apt-get)." >&2
  exit 1
fi

sudo apt-get update
sudo apt-get install -y \
  build-essential \
  cmake \
  clang \
  llvm-dev \
  curl \
  wget \
  file \
  pkg-config \
  libasound2-dev \
  libssl-dev \
  libclang-dev \
  patchelf \
  libxdo-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libgtk-3-dev \
  libpango1.0-dev \
  libgdk-pixbuf-2.0-dev \
  libatk1.0-dev \
  libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev \
  libwebkit2gtk-4.1-dev

echo "Linux dependencies installed. Verify with: cd src-tauri && cargo build"
