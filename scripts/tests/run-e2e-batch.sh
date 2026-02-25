#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ITERATIONS="${ITERATIONS:-10}"
WAIT_SECONDS="${WAIT_SECONDS:-30}"
PROMPT_STYLE="${PROMPT_STYLE:-compact}"
PARALLEL_AGENTS="${PARALLEL_AGENTS:-1}"
AGENTS_CSV="${AGENTS:-codex,claude}"
STOP_ON_FAILURE="${STOP_ON_FAILURE:-0}"
LOG_DIR="${LOG_DIR:-/tmp/agtmux-e2e-batch-$(date +%Y%m%d-%H%M%S)-$$}"

CODEX_WAIT_RUNNING_S_VALUE="${CODEX_WAIT_RUNNING_S:-}"
CODEX_WAIT_IDLE_S_VALUE="${CODEX_WAIT_IDLE_S:-}"
CLAUDE_WAIT_RUNNING_S_VALUE="${CLAUDE_WAIT_RUNNING_S:-}"
CLAUDE_WAIT_IDLE_S_VALUE="${CLAUDE_WAIT_IDLE_S:-}"

PASS_CODEX=0
FAIL_CODEX=0
PASS_CLAUDE=0
FAIL_CLAUDE=0
PASS_POLLER=0
FAIL_POLLER=0

mkdir -p "${LOG_DIR}"

log() {
  echo "[batch-e2e] $*"
}

fail() {
  echo "[batch-e2e][error] $*" >&2
  exit 1
}

trim() {
  local s="$1"
  s="${s#"${s%%[![:space:]]*}"}"
  s="${s%"${s##*[![:space:]]}"}"
  printf '%s' "${s}"
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

record_result() {
  local agent="$1"
  local status="$2"
  case "${agent}" in
    codex)
      if [ "${status}" -eq 0 ]; then PASS_CODEX=$((PASS_CODEX + 1)); else FAIL_CODEX=$((FAIL_CODEX + 1)); fi
      ;;
    claude)
      if [ "${status}" -eq 0 ]; then PASS_CLAUDE=$((PASS_CLAUDE + 1)); else FAIL_CLAUDE=$((FAIL_CLAUDE + 1)); fi
      ;;
    poller)
      if [ "${status}" -eq 0 ]; then PASS_POLLER=$((PASS_POLLER + 1)); else FAIL_POLLER=$((FAIL_POLLER + 1)); fi
      ;;
    *)
      fail "unsupported agent in record_result: ${agent}"
      ;;
  esac
}

run_single() {
  local agent="$1"
  local iter="$2"
  local log_file="${LOG_DIR}/${agent}-iter$(printf '%02d' "${iter}").log"
  local cmd=""

  case "${agent}" in
    codex)
      cmd="${ROOT_DIR}/scripts/tests/test-source-codex.sh"
      ;;
    claude)
      cmd="${ROOT_DIR}/scripts/tests/test-source-claude.sh"
      ;;
    poller)
      cmd="${ROOT_DIR}/scripts/tests/test-source-poller.sh"
      ;;
    *)
      fail "unsupported agent: ${agent}"
      ;;
  esac

  (
    cd "${ROOT_DIR}"
    WAIT_SECONDS="${WAIT_SECONDS}" \
    PROMPT_STYLE="${PROMPT_STYLE}" \
    CODEX_WAIT_RUNNING_S="${CODEX_WAIT_RUNNING_S_VALUE}" \
    CODEX_WAIT_IDLE_S="${CODEX_WAIT_IDLE_S_VALUE}" \
    CLAUDE_WAIT_RUNNING_S="${CLAUDE_WAIT_RUNNING_S_VALUE}" \
    CLAUDE_WAIT_IDLE_S="${CLAUDE_WAIT_IDLE_S_VALUE}" \
    bash "${cmd}"
  ) >"${log_file}" 2>&1
}

print_summary() {
  local total_codex=$((PASS_CODEX + FAIL_CODEX))
  local total_claude=$((PASS_CLAUDE + FAIL_CLAUDE))
  local total_poller=$((PASS_POLLER + FAIL_POLLER))
  local total_pass=$((PASS_CODEX + PASS_CLAUDE + PASS_POLLER))
  local total_fail=$((FAIL_CODEX + FAIL_CLAUDE + FAIL_POLLER))
  local total_all=$((total_codex + total_claude + total_poller))

  log "summary:"
  if [ "${total_codex}" -gt 0 ]; then
    log "  codex : ${PASS_CODEX}/${total_codex} pass"
  fi
  if [ "${total_claude}" -gt 0 ]; then
    log "  claude: ${PASS_CLAUDE}/${total_claude} pass"
  fi
  if [ "${total_poller}" -gt 0 ]; then
    log "  poller: ${PASS_POLLER}/${total_poller} pass"
  fi
  log "  total : ${total_pass}/${total_all} pass"
  log "logs: ${LOG_DIR}"

  if [ "${total_fail}" -gt 0 ]; then
    return 1
  fi
  return 0
}

main() {
  local agents=()
  local raw_agents=()
  local raw_agent=""
  local agent=""
  local iter=""
  local pid=""
  local status=0
  local has_failure=0
  local jobs=()

  require_cmd bash
  require_cmd just

  if ! [[ "${ITERATIONS}" =~ ^[0-9]+$ ]] || [ "${ITERATIONS}" -lt 1 ]; then
    fail "ITERATIONS must be a positive integer"
  fi

  IFS=',' read -r -a raw_agents <<< "${AGENTS_CSV}"
  for raw_agent in "${raw_agents[@]}"; do
    [ -n "${raw_agent}" ] || continue
    agent="$(trim "${raw_agent}")"
    [ -n "${agent}" ] || continue
    agents+=("${agent}")
  done
  [ "${#agents[@]}" -gt 0 ] || fail "no agents selected via AGENTS='${AGENTS_CSV}'"

  log "preflight start"
  (cd "${ROOT_DIR}" && just preflight-online)
  log "preflight pass"

  for iter in $(seq 1 "${ITERATIONS}"); do
    log "iteration ${iter}/${ITERATIONS}"
    jobs=()
    if [ "${PARALLEL_AGENTS}" = "1" ] && [ "${#agents[@]}" -gt 1 ]; then
      for agent in "${agents[@]}"; do
        run_single "${agent}" "${iter}" &
        pid=$!
        jobs+=("${agent}:${pid}")
      done
      for job in "${jobs[@]}"; do
        agent="${job%%:*}"
        pid="${job##*:}"
        if wait "${pid}"; then
          status=0
        else
          status=$?
        fi
        record_result "${agent}" "${status}"
        if [ "${status}" -eq 0 ]; then
          log "  ${agent}: PASS"
        else
          log "  ${agent}: FAIL (see ${LOG_DIR}/${agent}-iter$(printf '%02d' "${iter}").log)"
          has_failure=1
        fi
      done
    else
      for agent in "${agents[@]}"; do
        if run_single "${agent}" "${iter}"; then
          status=0
        else
          status=$?
        fi
        record_result "${agent}" "${status}"
        if [ "${status}" -eq 0 ]; then
          log "  ${agent}: PASS"
        else
          log "  ${agent}: FAIL (see ${LOG_DIR}/${agent}-iter$(printf '%02d' "${iter}").log)"
          has_failure=1
        fi
      done
    fi

    if [ "${STOP_ON_FAILURE}" = "1" ] && [ "${has_failure}" -eq 1 ]; then
      log "stop-on-failure enabled; aborting remaining iterations"
      break
    fi
  done

  if print_summary; then
    return 0
  fi
  return 1
}

main "$@"
