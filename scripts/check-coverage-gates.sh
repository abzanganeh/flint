#!/usr/bin/env bash
# Phase 7.1 — enforce flint-testing.mdc module floors from llvm-cov summary.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}/src-tauri"

SUMMARY="$(cargo llvm-cov test --lib --tests --summary-only 2>&1)"
echo "${SUMMARY}" | tail -30

check_module_floor() {
  local module="$1"
  local floor="$2"
  local pct
  pct="$(echo "${SUMMARY}" | awk -v m="${module}" '$1 == m { gsub(/%/, "", $4); print $4; exit }')"
  if [[ -z "${pct}" ]]; then
    echo "coverage gate: module ${module} not found in llvm-cov summary" >&2
    exit 1
  fi
  awk -v p="${pct}" -v f="${floor}" 'BEGIN { if (p+0 < f+0) exit 1 }' || {
    echo "coverage gate FAIL: ${module} ${pct}% < ${floor}% floor" >&2
    exit 1
  }
  echo "coverage gate OK: ${module} ${pct}% (floor ${floor}%)"
}

# flint-testing.mdc targets (interim floors — ratchet as integration coverage grows)
check_module_floor "session/state.rs" 95
check_module_floor "confidence.rs" 85
check_module_floor "session/memory.rs" 85
check_module_floor "llm/rate_limiter.rs" 85
check_module_floor "transcription/detector.rs" 85
check_module_floor "rag/retriever.rs" 10
check_module_floor "orchestrator/prewarm.rs" 10

# Repo-wide regression guard (~53% today with lib + integration tests)
TOTAL_PCT="$(echo "${SUMMARY}" | awk '/^TOTAL/ { gsub(/%/, "", $4); print $4; exit }')"
awk -v p="${TOTAL_PCT}" 'BEGIN { if (p+0 < 50) exit 1 }' || {
  echo "coverage gate FAIL: TOTAL ${TOTAL_PCT}% < 50% repo floor" >&2
  exit 1
}
echo "coverage gate OK: TOTAL ${TOTAL_PCT}% (floor 50%)"
