#!/usr/bin/env bash
# dev-clean.sh — stop stale Flint/Vite processes, optionally clean PipeWire
# loopback, load .env, and start a fresh `npm run tauri dev`.
#
# Usage:
#   ./scripts/dev-clean.sh              # normal clean start
#   ./scripts/dev-clean.sh --verbose    # RUST_LOG=info,flint::audio::chunk=trace
#   ./scripts/dev-clean.sh --skip-audio # skip PipeWire loopback cleanup
#   ./scripts/dev-clean.sh -- <args>    # forward extra args to tauri dev
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

VERBOSE=false
SKIP_AUDIO=false
TAURI_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --verbose|-v)
      VERBOSE=true
      shift
      ;;
    --skip-audio)
      SKIP_AUDIO=true
      shift
      ;;
    --)
      shift
      TAURI_ARGS=("$@")
      break
      ;;
    -h|--help)
      sed -n '2,10p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      TAURI_ARGS+=("$1")
      shift
      ;;
  esac
done

echo "==> Stopping stale Flint / Vite processes..."
pkill -f "${ROOT}.*tauri dev" 2>/dev/null || true
pkill -f "${ROOT}.*vite" 2>/dev/null || true
pkill -f "${ROOT}/src-tauri/target/debug/flint" 2>/dev/null || true
# Give processes a moment to exit before rebinding ports.
sleep 0.5

if [[ "${SKIP_AUDIO}" == false ]] && [[ -x "${ROOT}/scripts/cleanup-pipewire-loopback.sh" ]]; then
  echo "==> Checking PipeWire loopback modules..."
  bash "${ROOT}/scripts/cleanup-pipewire-loopback.sh" || true
fi

if [[ -f "${ROOT}/.env" ]]; then
  echo "==> Loading ${ROOT}/.env"
  set -a
  # shellcheck disable=SC1091
  source "${ROOT}/.env"
  set +a
else
  echo "==> No .env found (copy .env.example if Supabase auth is needed)"
fi

if [[ "${VERBOSE}" == true ]]; then
  export RUST_LOG="${RUST_LOG:-info,flint::audio::chunk=trace,flint_lib::audio=trace}"
  echo "==> RUST_LOG=${RUST_LOG}"
fi

echo "==> Starting npm run tauri dev"
if [[ ${#TAURI_ARGS[@]} -gt 0 ]]; then
  exec npm run tauri dev -- "${TAURI_ARGS[@]}"
else
  exec npm run tauri dev
fi
