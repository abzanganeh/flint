#!/usr/bin/env bash
# run-dev-stack.sh — start Smart Resume, Flint desktop, and the browser extension together.
#
# Expected layout (sibling repos under the same parent directory):
#   ../smart-resume/
#   ../flint-extension/
#   ./  (Flint)
#
# Usage:
#   ./scripts/run-dev-stack.sh                 # dockerized Smart Resume + extension + Flint
#   ./scripts/run-dev-stack.sh --resume-local  # native uv/pnpm Smart Resume instead of Docker
#   ./scripts/run-dev-stack.sh --with-supabase # start local Supabase before Flint
#   ./scripts/run-dev-stack.sh --skip-flint    # resume + extension only
#   ./scripts/run-dev-stack.sh --skip-extension
#   ./scripts/run-dev-stack.sh --skip-resume
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECTS_ROOT="$(cd "${ROOT}/.." && pwd)"
SMART_RESUME_ROOT="${SMART_RESUME_ROOT:-${PROJECTS_ROOT}/smart-resume}"
EXTENSION_ROOT="${EXTENSION_ROOT:-${PROJECTS_ROOT}/flint-extension}"
LOG_DIR="${HOME}/.flint/dev-stack"
RESUME_MODE="docker"
WITH_SUPABASE=false
SKIP_RESUME=false
SKIP_EXTENSION=false
SKIP_FLINT=false

PIDS=()
COMPOSE_STARTED=false

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \?//'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --resume-docker)
      RESUME_MODE="docker"
      shift
      ;;
    --resume-local)
      RESUME_MODE="local"
      shift
      ;;
    --with-supabase)
      WITH_SUPABASE=true
      shift
      ;;
    --skip-resume)
      SKIP_RESUME=true
      shift
      ;;
    --skip-extension)
      SKIP_EXTENSION=true
      shift
      ;;
    --skip-flint)
      SKIP_FLINT=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

mkdir -p "${LOG_DIR}"

log() {
  printf '==> %s\n' "$*"
}

warn() {
  printf 'WARN: %s\n' "$*" >&2
}

require_dir() {
  local dir="$1"
  local label="$2"
  if [[ ! -d "${dir}" ]]; then
    echo "ERROR: ${label} not found at ${dir}" >&2
    echo "Set SMART_RESUME_ROOT / EXTENSION_ROOT or clone repos as siblings of Flint." >&2
    exit 1
  fi
}

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "ERROR: required command not found: ${cmd}" >&2
    exit 1
  fi
}

wait_for_url() {
  local url="$1"
  local label="$2"
  local timeout="${3:-180}"
  local elapsed=0

  log "Waiting for ${label} (${url})..."
  while (( elapsed < timeout )); do
    if curl -sf "${url}" >/dev/null 2>&1; then
      log "${label} is ready"
      return 0
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done

  echo "ERROR: ${label} did not become ready within ${timeout}s" >&2
  echo "Check logs in ${LOG_DIR}" >&2
  return 1
}

track_pid() {
  PIDS+=("$1")
}

stop_pid() {
  local pid="$1"
  if kill -0 "${pid}" 2>/dev/null; then
    kill "${pid}" 2>/dev/null || true
    wait "${pid}" 2>/dev/null || true
  fi
}

cleanup() {
  local pid
  log "Stopping dev stack..."
  for pid in "${PIDS[@]}"; do
    stop_pid "${pid}"
  done

  if [[ "${COMPOSE_STARTED}" == true ]]; then
    (
      cd "${SMART_RESUME_ROOT}"
      docker compose down >/dev/null 2>&1 || true
    )
  fi
}

trap cleanup EXIT INT TERM

start_background() {
  local name="$1"
  local log_file="$2"
  shift 2

  log "Starting ${name} (log: ${log_file})"
  "$@" >>"${log_file}" 2>&1 &
  track_pid "$!"
}

start_resume_docker() {
  require_cmd docker
  require_dir "${SMART_RESUME_ROOT}" "Smart Resume"

  if [[ ! -f "${SMART_RESUME_ROOT}/backend/.env" ]]; then
    warn "Missing ${SMART_RESUME_ROOT}/backend/.env — copy backend/.env.example and fill required vars."
  fi

  log "Starting Smart Resume via Docker Compose"
  (
    cd "${SMART_RESUME_ROOT}"
    docker compose up -d --build
  )
  COMPOSE_STARTED=true

  wait_for_url "http://localhost:8000/docs" "Smart Resume API"
  wait_for_url "http://localhost:3000" "Smart Resume web app"
}

start_resume_local() {
  require_dir "${SMART_RESUME_ROOT}" "Smart Resume"
  require_cmd uv

  if [[ ! -f "${SMART_RESUME_ROOT}/backend/.env" ]]; then
    warn "Missing ${SMART_RESUME_ROOT}/backend/.env — copy backend/.env.example and fill required vars."
  fi

  start_background "Smart Resume backend" "${LOG_DIR}/smart-resume-backend.log" \
    bash -lc "cd '${SMART_RESUME_ROOT}/backend' && uv run uvicorn app.main:app --reload --port 8000"

  if command -v pnpm >/dev/null 2>&1; then
    start_background "Smart Resume frontend" "${LOG_DIR}/smart-resume-frontend.log" \
      bash -lc "cd '${SMART_RESUME_ROOT}/frontend' && pnpm dev"
  elif command -v npm >/dev/null 2>&1; then
    start_background "Smart Resume frontend" "${LOG_DIR}/smart-resume-frontend.log" \
      bash -lc "cd '${SMART_RESUME_ROOT}/frontend' && npm run dev"
  else
    echo "ERROR: pnpm or npm required for local Smart Resume frontend" >&2
    exit 1
  fi

  wait_for_url "http://localhost:8000/docs" "Smart Resume API"
  wait_for_url "http://localhost:3000" "Smart Resume web app"
}

start_extension() {
  require_dir "${EXTENSION_ROOT}" "Flint extension"
  require_cmd npm

  if [[ ! -d "${EXTENSION_ROOT}/node_modules" ]]; then
    warn "Extension node_modules missing — run: cd ${EXTENSION_ROOT} && npm install"
  fi

  if [[ ! -f "${EXTENSION_ROOT}/.env" ]]; then
    warn "Missing ${EXTENSION_ROOT}/.env — copy .env.example if API URLs differ from defaults."
  fi

  start_background "Flint extension (watch build)" "${LOG_DIR}/flint-extension.log" \
    bash -lc "cd '${EXTENSION_ROOT}' && npm run dev"

  log "Extension builds to ${EXTENSION_ROOT}/dist — load unpacked in chrome://extensions"
}

start_flint() {
  require_cmd npm

  if [[ "${WITH_SUPABASE}" == true ]]; then
    if command -v supabase >/dev/null 2>&1; then
      log "Starting local Supabase"
      (
        cd "${ROOT}"
        npm run supabase:start
      )
    else
      warn "Supabase CLI not found; skipping --with-supabase"
    fi
  fi

  if [[ -f "${ROOT}/.env" ]]; then
    log "Loading ${ROOT}/.env for Flint"
    set -a
    # shellcheck disable=SC1091
    source "${ROOT}/.env"
    set +a
  else
    warn "Missing ${ROOT}/.env — copy .env.example (Supabase + Smart Resume URL)."
  fi

  log "Starting Flint desktop (foreground — Ctrl+C stops the whole stack)"
  log "Flint UI: http://127.0.0.1:1420"
  (
    cd "${ROOT}"
    exec npm run tauri dev
  )
}

main() {
  log "Flint dev stack"
  log "Projects root: ${PROJECTS_ROOT}"
  log "Logs: ${LOG_DIR}"

  if [[ "${SKIP_RESUME}" == false ]]; then
    if [[ "${RESUME_MODE}" == "docker" ]]; then
      start_resume_docker
    else
      start_resume_local
    fi
  else
    log "Skipping Smart Resume"
  fi

  if [[ "${SKIP_EXTENSION}" == false ]]; then
    start_extension
  else
    log "Skipping browser extension"
  fi

  if [[ "${SKIP_FLINT}" == false ]]; then
    start_flint
  else
    log "Flint skipped — background services still running. Press Ctrl+C to stop."
    wait
  fi
}

main "$@"
