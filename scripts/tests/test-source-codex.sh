#!/usr/bin/env bash
set -euo pipefail

SESSION="agtmux-e2e-codex-$(date +%s)-$$"
WINDOW="e2e"
KEEP_TMUX_VALUE="${KEEP_TMUX:-0}"
KEEP_WORKDIR_VALUE="${KEEP_WORKDIR:-0}"
PANE_ID=""
PANE_PID=""
WORKDIR=""
WAIT_SECONDS="${WAIT_SECONDS:-60}"
PROMPT_STYLE="${PROMPT_STYLE:-strict}"
case "${WAIT_SECONDS}" in
  30)
    WAIT_PHRASE="wait 30 seconds by using sleep 30"
    DEFAULT_WAIT_RUNNING_S=20
    DEFAULT_WAIT_IDLE_S=90
    ;;
  60)
    WAIT_PHRASE="wait 60 seconds by using sleep 60"
    DEFAULT_WAIT_RUNNING_S=50
    DEFAULT_WAIT_IDLE_S=180
    ;;
  *)
    echo "[codex-e2e][error] unsupported WAIT_SECONDS='${WAIT_SECONDS}' (expected 30 or 60)" >&2
    exit 1
    ;;
esac
CODEX_WAIT_RUNNING_S="${CODEX_WAIT_RUNNING_S:-${DEFAULT_WAIT_RUNNING_S}}"
CODEX_WAIT_IDLE_S="${CODEX_WAIT_IDLE_S:-${DEFAULT_WAIT_IDLE_S}}"
CODEX_MODEL_VALUE="${CODEX_MODEL:-gpt-5.3-codex}"
CODEX_EFFORT_VALUE="${CODEX_EFFORT:-medium}"
CODEX_EXEC_BASE_CMD="codex exec --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --json --model ${CODEX_MODEL_VALUE} -c model_reasoning_effort=\"${CODEX_EFFORT_VALUE}\""
# v4 wait_prompt_template.rs equivalent for wait=60.
if [ "${PROMPT_STYLE}" = "compact" ]; then
  WAIT_PROMPT="Run exactly one bash command. ${WAIT_PHRASE}. bash -lc 'sleep ${WAIT_SECONDS}; printf \"wait_result=idle\\n\"'. Output exactly one non-empty line: wait_result=idle."
else
  WAIT_PROMPT="Run exactly one bash command and do not run any additional commands. ${WAIT_PHRASE}. bash -lc 'sleep ${WAIT_SECONDS}; printf \"wait_result=idle\\n\"' Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=idle"
fi

log() {
  echo "[codex-e2e] $*"
}

fail() {
  echo "[codex-e2e][error] $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

create_workspace() {
  WORKDIR="$(mktemp -d "/tmp/agtmux-e2e-codex-XXXXXX")" || fail "failed to create temp workspace"
  log "created isolated workspace: ${WORKDIR}"
  git -C "${WORKDIR}" init -q || fail "failed to init git repo in temp workspace"
}

collect_descendants() {
  local root_pid="$1"
  local queue=("${root_pid}")
  local current_pid=""
  local child_pid=""
  local descendants=()

  command -v pgrep >/dev/null 2>&1 || return 0

  while [ "${#queue[@]}" -gt 0 ]; do
    current_pid="${queue[0]}"
    queue=("${queue[@]:1}")
    while IFS= read -r child_pid; do
      [ -n "${child_pid}" ] || continue
      descendants+=("${child_pid}")
      queue+=("${child_pid}")
    done < <(pgrep -P "${current_pid}" 2>/dev/null || true)
  done

  if [ "${#descendants[@]}" -gt 0 ]; then
    printf '%s\n' "${descendants[@]}"
  fi
}

kill_pane_children() {
  local descendant_pid=""
  local descendants=()
  local alive=()

  [ -n "${PANE_PID}" ] || return 0

  while IFS= read -r descendant_pid; do
    [ -n "${descendant_pid}" ] || continue
    descendants+=("${descendant_pid}")
  done < <(collect_descendants "${PANE_PID}" || true)

  if [ "${#descendants[@]}" -eq 0 ]; then
    return 0
  fi

  log "terminating remaining pane child processes: ${descendants[*]}"
  kill "${descendants[@]}" 2>/dev/null || true
  sleep 1

  for descendant_pid in "${descendants[@]}"; do
    if kill -0 "${descendant_pid}" 2>/dev/null; then
      alive+=("${descendant_pid}")
    fi
  done

  if [ "${#alive[@]}" -gt 0 ]; then
    log "force-killing pane child processes: ${alive[*]}"
    kill -9 "${alive[@]}" 2>/dev/null || true
  fi
}

cleanup() {
  local exit_code=$?
  trap - EXIT

  if [ "${KEEP_TMUX_VALUE}" = "1" ]; then
    log "KEEP_TMUX=1; leaving tmux session '${SESSION}'"
  else
    kill_pane_children
    if tmux has-session -t "${SESSION}" 2>/dev/null; then
      log "killing tmux session '${SESSION}'"
      tmux kill-session -t "${SESSION}" || true
    fi
  fi

  if [ -n "${WORKDIR}" ]; then
    if [ "${KEEP_WORKDIR_VALUE}" = "1" ]; then
      log "KEEP_WORKDIR=1; leaving temp workspace '${WORKDIR}'"
    else
      log "removing temp workspace '${WORKDIR}'"
      rm -rf "${WORKDIR}" || true
    fi
  fi

  exit "${exit_code}"
}

capture_text() {
  tmux capture-pane -p -t "${PANE_ID}" -S -200
}

pane_cmd() {
  tmux display-message -p -t "${PANE_ID}" "#{pane_current_command}"
}

print_capture() {
  local label="$1"
  local text="$2"
  echo "===== CODEX CAPTURE: ${label} ====="
  printf '%s\n' "${text}"
  echo "===== END CODEX CAPTURE: ${label} ====="
}

send_line() {
  local text="$1"
  tmux send-keys -t "${PANE_ID}" -l "${text}" || fail "failed to send keys"
  tmux send-keys -t "${PANE_ID}" C-m || fail "failed to send enter"
}

cd_to_workspace() {
  local escaped_workdir=""
  printf -v escaped_workdir '%q' "${WORKDIR}"
  log "switching tmux pane to isolated workspace"
  send_line "cd ${escaped_workdir}"
  sleep 1
}

has_obvious_launch_failure() {
  local text="$1"
  printf '%s\n' "${text}" | grep -Eiq "command not found|not recognized as an internal|No such file or directory"
}

has_idle_marker() {
  local text="$1"
  printf '%s\n' "${text}" | grep -Eq "wait_result=idle"
}

has_turn_completed() {
  local text="$1"
  printf '%s\n' "${text}" | grep -Eq "\"type\":\"turn.completed\""
}

main() {
  trap cleanup EXIT
  require_cmd tmux
  require_cmd codex
  require_cmd git
  require_cmd mktemp
  create_workspace

  log "creating tmux session ${SESSION}:${WINDOW}"
  tmux new-session -d -s "${SESSION}" -n "${WINDOW}" || fail "tmux new-session failed"
  PANE_ID="$(tmux list-panes -t "${SESSION}:${WINDOW}" -F '#{pane_id}' | head -n1)"
  [ -n "${PANE_ID}" ] || fail "failed to resolve pane id"
  PANE_PID="$(tmux display-message -p -t "${PANE_ID}" '#{pane_pid}')"
  [ -n "${PANE_PID}" ] || fail "failed to resolve pane pid"

  cd_to_workspace

  log "recording model/effort selection marker"
  send_line "printf 'codex_model=%s codex_effort=%s\\n' '${CODEX_MODEL_VALUE}' '${CODEX_EFFORT_VALUE}'"

  log "launching codex exec (v4 required-flags equivalent)"
  ESCAPED_PROMPT=""
  printf -v ESCAPED_PROMPT '%q' "${WAIT_PROMPT}"
  send_line "${CODEX_EXEC_BASE_CMD} ${ESCAPED_PROMPT}"
  sleep 8

  START_CAPTURE="$(capture_text)" || fail "capture-pane failed during launch check"
  print_capture "post-launch" "${START_CAPTURE}"
  if has_obvious_launch_failure "${START_CAPTURE}"; then
    fail "codex launch appears to have failed"
  fi
  if ! printf '%s\n' "${START_CAPTURE}" | grep -Fq "codex_model=${CODEX_MODEL_VALUE} codex_effort=${CODEX_EFFORT_VALUE}"; then
    fail "model/effort marker missing from codex capture"
  fi

  log "sending wait prompt template (${WAIT_SECONDS}s, style=${PROMPT_STYLE})"
  log "observing around t+${CODEX_WAIT_RUNNING_S}s (running window)"
  sleep "${CODEX_WAIT_RUNNING_S}"
  CAPTURE_RUNNING="$(capture_text)" || fail "capture-pane failed at t+${CODEX_WAIT_RUNNING_S}s"
  CMD_RUNNING="$(pane_cmd)"
  print_capture "t+${CODEX_WAIT_RUNNING_S}s (running window)" "${CAPTURE_RUNNING}"
  if [[ "${CMD_RUNNING}" == *codex* || "${CMD_RUNNING}" == *node* ]]; then
    log "observed running command at t+${CODEX_WAIT_RUNNING_S}s (pane_current_command=${CMD_RUNNING})"
  else
    fail "expected codex to be running at t+${CODEX_WAIT_RUNNING_S}s, got pane_current_command='${CMD_RUNNING}'"
  fi

  log "observing around t+${CODEX_WAIT_IDLE_S}s (idle window)"
  sleep "$((CODEX_WAIT_IDLE_S - CODEX_WAIT_RUNNING_S))"
  CAPTURE_IDLE="$(capture_text)" || fail "capture-pane failed at t+${CODEX_WAIT_IDLE_S}s"
  CMD_IDLE="$(pane_cmd)"
  print_capture "t+${CODEX_WAIT_IDLE_S}s (idle window)" "${CAPTURE_IDLE}"

  if [[ "${CMD_IDLE}" == *codex* || "${CMD_IDLE}" == *node* ]]; then
    fail "codex still running at t+${CODEX_WAIT_IDLE_S}s (pane_current_command='${CMD_IDLE}')"
  fi
  if ! has_idle_marker "${CAPTURE_IDLE}"; then
    fail "missing idle marker in codex output: wait_result=idle"
  fi
  if ! has_turn_completed "${CAPTURE_IDLE}"; then
    fail "missing turn completion marker in codex output"
  fi
  log "observed stable codex lifecycle (running at t+${CODEX_WAIT_RUNNING_S}s, idle marker by t+${CODEX_WAIT_IDLE_S}s)"
}

main "$@"
