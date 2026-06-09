#!/usr/bin/env bash
# Linux dev handler for flint:// — used by register-flint-deeplink-linux.sh
#
# Cold start: start a single `npm run tauri dev` with FLINT_IMPORT_URL (no second
# binary — avoids a blank window restoring SQLite while the import URL is lost).
# Warm path: Vite already up → exec once; single-instance forwards the URL.
set -euo pipefail

URL="${1:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${ROOT}/src-tauri/target/debug/flint"
VITE_URL="${FLINT_VITE_URL:-http://localhost:1420}"
LOG="${HOME}/.flint/handler.log"
LOCK="${HOME}/.flint/handler.lock"
DEBOUNCE="${HOME}/.flint/handler.last"

log() {
  echo "$(date -Is) $*" >> "${LOG}"
}

vite_up() {
  curl -sf "${VITE_URL}/" >/dev/null 2>&1
}

wait_for_vite() {
  local i
  for i in $(seq 1 120); do
    if vite_up; then
      return 0
    fi
    sleep 1
  done
  return 1
}

start_tauri_dev_with_import() {
  log "cold start: launching tauri dev with deep link arg"
  echo "[flint-deeplink] Starting Flint (first launch may take ~30s)…" >&2
  (
    cd "${ROOT}"
    export FLINT_IMPORT_URL="${URL}"
    nohup npm run tauri dev -- "${URL}" >>"${HOME}/.flint/tauri-dev.log" 2>&1 &
  )
}

mkdir -p "${HOME}/.flint"

if [[ -z "${URL}" ]]; then
  log "empty url; abort"
  exit 1
fi

# Debounce duplicate browser/handler invocations (popup + fallback).
if [[ -f "${DEBOUNCE}" ]]; then
  last="$(cat "${DEBOUNCE}" 2>/dev/null || true)"
  if [[ "${last}" == "${URL}" ]]; then
    log "duplicate url skipped url=${URL}"
    exit 0
  fi
fi
echo "${URL}" > "${DEBOUNCE}"

exec 9>"${LOCK}"
if ! flock -n 9; then
  log "handler locked; skipping url=${URL}"
  exit 0
fi

log "handler invoked url=${URL}"

if [[ ! -x "${BINARY}" ]]; then
  echo "Flint binary missing. Run once: cd ${ROOT} && npm run tauri dev" >&2
  exit 1
fi

if vite_up; then
  log "vite up; single-instance exec"
  exec "${BINARY}" "${URL}"
fi

if pgrep -f "${ROOT}.*tauri dev" >/dev/null 2>&1; then
  log "tauri dev starting; waiting for vite"
  if wait_for_vite; then
    log "vite ready after wait; single-instance exec"
    exec "${BINARY}" "${URL}"
  fi
  log "vite wait timed out while tauri dev booting"
  exit 1
fi

start_tauri_dev_with_import
exit 0
