#!/usr/bin/env bash
set -euo pipefail

# Tauri 2.x macOS build dependencies for Flint.
# Run once on a fresh dev or CI macOS machine: ./scripts/install-macos-deps.sh
#
# Xcode Command Line Tools provide clang/SDKs. cmake powers whisper-rs's
# whisper.cpp build; llvm via brew gives us libclang for bindgen.

if ! command -v brew >/dev/null 2>&1; then
  echo "Homebrew is required (see https://brew.sh)." >&2
  exit 1
fi

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Installing Xcode Command Line Tools (may prompt for interaction)..."
  xcode-select --install || true
fi

brew update
brew install cmake llvm pkg-config

LLVM_PREFIX="$(brew --prefix llvm)"
LIBCLANG_PATH="${LLVM_PREFIX}/lib"

if [[ ! -e "${LIBCLANG_PATH}/libclang.dylib" ]]; then
  echo "libclang.dylib not found at ${LIBCLANG_PATH}" >&2
  exit 1
fi

echo "LIBCLANG_PATH=${LIBCLANG_PATH}"

# Surface env to GitHub Actions when running under CI.
if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "LIBCLANG_PATH=${LIBCLANG_PATH}" >> "${GITHUB_ENV}"
fi

echo "macOS dependencies installed. Verify with: cd src-tauri && cargo build"
