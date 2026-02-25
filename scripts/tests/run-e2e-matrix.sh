#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ITERATIONS_PER_CASE="${ITERATIONS_PER_CASE:-2}"
PARALLEL_CASES="${PARALLEL_CASES:-1}"
LOG_ROOT="${LOG_ROOT:-/tmp/agtmux-e2e-matrix-$(date +%Y%m%d-%H%M%S)-$$}"

mkdir -p "${LOG_ROOT}"

log() {
  echo "[matrix-e2e] $*"
}

run_case() {
  local case_name="$1"
  shift
  local case_log_dir="${LOG_ROOT}/${case_name}"
  mkdir -p "${case_log_dir}"
  (
    cd "${ROOT_DIR}"
    env \
      ITERATIONS="${ITERATIONS_PER_CASE}" \
      AGENTS="codex,claude" \
      PARALLEL_AGENTS="1" \
      LOG_DIR="${case_log_dir}" \
      "$@" \
      bash scripts/tests/run-e2e-batch.sh
  )
}

main() {
  local pids=()
  local names=()
  local i=0
  local status=0
  local failed=0
  local pid=""
  local name=""

  log "log root: ${LOG_ROOT}"

  if [ "${PARALLEL_CASES}" = "1" ]; then
    run_case \
      "fast-compact" \
      WAIT_SECONDS=30 PROMPT_STYLE=compact CODEX_WAIT_RUNNING_S=18 CODEX_WAIT_IDLE_S=80 CLAUDE_WAIT_RUNNING_S=12 CLAUDE_WAIT_IDLE_S=55 \
      >"${LOG_ROOT}/fast-compact.stdout.log" 2>&1 &
    pids+=("$!")
    names+=("fast-compact")

    run_case \
      "conservative-strict" \
      WAIT_SECONDS=30 PROMPT_STYLE=strict CODEX_WAIT_RUNNING_S=22 CODEX_WAIT_IDLE_S=95 CLAUDE_WAIT_RUNNING_S=15 CLAUDE_WAIT_IDLE_S=70 \
      >"${LOG_ROOT}/conservative-strict.stdout.log" 2>&1 &
    pids+=("$!")
    names+=("conservative-strict")
  else
    run_case \
      "fast-compact" \
      WAIT_SECONDS=30 PROMPT_STYLE=compact CODEX_WAIT_RUNNING_S=18 CODEX_WAIT_IDLE_S=80 CLAUDE_WAIT_RUNNING_S=12 CLAUDE_WAIT_IDLE_S=55 \
      >"${LOG_ROOT}/fast-compact.stdout.log" 2>&1
    run_case \
      "conservative-strict" \
      WAIT_SECONDS=30 PROMPT_STYLE=strict CODEX_WAIT_RUNNING_S=22 CODEX_WAIT_IDLE_S=95 CLAUDE_WAIT_RUNNING_S=15 CLAUDE_WAIT_IDLE_S=70 \
      >"${LOG_ROOT}/conservative-strict.stdout.log" 2>&1
  fi

  if [ "${PARALLEL_CASES}" = "1" ]; then
    for i in "${!pids[@]}"; do
      pid="${pids[$i]}"
      name="${names[$i]}"
      if wait "${pid}"; then
        log "${name}: PASS"
      else
        status=$?
        log "${name}: FAIL (status=${status})"
        failed=1
      fi
    done
  fi

  log "matrix logs: ${LOG_ROOT}"
  if [ "${failed}" -eq 1 ]; then
    return 1
  fi
  return 0
}

main "$@"
