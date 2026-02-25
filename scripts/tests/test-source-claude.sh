#!/usr/bin/env bash
set -euo pipefail

SESSION="agtmux-e2e-claude-$(date +%s)-$$"
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
    DEFAULT_WAIT_RUNNING_S=15
    DEFAULT_WAIT_IDLE_S=70
    ;;
  60)
    WAIT_PHRASE="wait 60 seconds by using sleep 60"
    DEFAULT_WAIT_RUNNING_S=40
    DEFAULT_WAIT_IDLE_S=120
    ;;
  *)
    echo "[claude-e2e][error] unsupported WAIT_SECONDS='${WAIT_SECONDS}' (expected 30 or 60)" >&2
    exit 1
    ;;
esac
CLAUDE_WAIT_RUNNING_S="${CLAUDE_WAIT_RUNNING_S:-${DEFAULT_WAIT_RUNNING_S}}"
CLAUDE_WAIT_IDLE_S="${CLAUDE_WAIT_IDLE_S:-${DEFAULT_WAIT_IDLE_S}}"
CLAUDE_MODEL_VALUE="${CLAUDE_MODEL:-claude-sonnet-4-6}"
CLAUDE_LAUNCH_CMD="claude --dangerously-skip-permissions --model ${CLAUDE_MODEL_VALUE}"
# v4 wait_prompt_template.rs equivalent for wait=60.
if [ "${PROMPT_STYLE}" = "compact" ]; then
  WAIT_PROMPT="Run exactly one bash command. ${WAIT_PHRASE}. bash -lc 'sleep ${WAIT_SECONDS}; printf \"wait_result=%s\\n\" \"<running|idle>\"'. Replace <running|idle> with the observed state. Output exactly one non-empty line: wait_result=<running|idle>."
else
  WAIT_PROMPT="Run exactly one bash command and do not run any additional commands. ${WAIT_PHRASE}. bash -lc 'sleep ${WAIT_SECONDS}; printf \"wait_result=%s\\n\" \"<running|idle>\"' Use the same command shape, replacing <running|idle> with the observed state after the wait. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=<running|idle>"
fi

log() {
  echo "[claude-e2e] $*"
}

fail() {
  echo "[claude-e2e][error] $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

create_workspace() {
  WORKDIR="$(mktemp -d "/tmp/agtmux-e2e-claude-XXXXXX")" || fail "failed to create temp workspace"
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

print_capture() {
  local label="$1"
  local text="$2"
  echo "===== CLAUDE CAPTURE: ${label} ====="
  printf '%s\n' "${text}"
  echo "===== END CLAUDE CAPTURE: ${label} ====="
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

has_workspace_trust_gate() {
  local text="$1"
  printf '%s\n' "${text}" | grep -Eiq "Do you trust the contents of this directory|Yes, continue|Quick safety check|Yes, I trust this folder"
}

has_running_marker() {
  local text="$1"
  printf '%s\n' "${text}" | grep -Eq "Running…|Running\\.\\.\\.|Running\\.|Bash\\(bash -lc 'sleep"
}

has_idle_marker() {
  local text="$1"
  printf '%s\n' "${text}" | grep -Eq "wait_result=idle"
}

main() {
  trap cleanup EXIT
  require_cmd tmux
  require_cmd claude
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

  log "recording model selection marker"
  send_line "printf 'claude_model=%s\\n' '${CLAUDE_MODEL_VALUE}'"

  log "launching claude interactive CLI (v4 required-flags equivalent)"
  send_line "${CLAUDE_LAUNCH_CMD}"
  sleep 6

  LAUNCH_CAPTURE="$(capture_text)" || fail "capture-pane failed during launch check"
  print_capture "post-launch" "${LAUNCH_CAPTURE}"
  if has_obvious_launch_failure "${LAUNCH_CAPTURE}"; then
    fail "claude launch appears to have failed"
  fi
  if ! printf '%s\n' "${LAUNCH_CAPTURE}" | grep -Fq "claude_model=${CLAUDE_MODEL_VALUE}"; then
    fail "model marker missing from claude capture"
  fi
  if has_workspace_trust_gate "${LAUNCH_CAPTURE}"; then
    log "workspace trust gate detected; sending Enter to continue"
    tmux send-keys -t "${PANE_ID}" C-m || fail "failed to accept workspace trust gate"
    sleep 2
    LAUNCH_CAPTURE="$(capture_text)" || fail "capture-pane failed after trust gate handling"
    print_capture "post-trust-ack" "${LAUNCH_CAPTURE}"
  fi

  log "sending wait prompt template (${WAIT_SECONDS}s, style=${PROMPT_STYLE})"
  send_line "${WAIT_PROMPT}"

  log "observing around t+${CLAUDE_WAIT_RUNNING_S}s (running window)"
  CAPTURE_RUNNING=""
  sleep "${CLAUDE_WAIT_RUNNING_S}"
  CAPTURE_RUNNING="$(capture_text)" || fail "capture-pane failed at t+${CLAUDE_WAIT_RUNNING_S}s"
  print_capture "t+${CLAUDE_WAIT_RUNNING_S}s (running window)" "${CAPTURE_RUNNING}"
  if ! has_running_marker "${CAPTURE_RUNNING}"; then
    fail "missing running marker in claude capture"
  fi

  log "observing around t+${CLAUDE_WAIT_IDLE_S}s (idle window)"
  CAPTURE_IDLE=""
  sleep "$((CLAUDE_WAIT_IDLE_S - CLAUDE_WAIT_RUNNING_S))"
  CAPTURE_IDLE="$(capture_text)" || fail "capture-pane failed at t+${CLAUDE_WAIT_IDLE_S}s"
  print_capture "t+${CLAUDE_WAIT_IDLE_S}s (idle window)" "${CAPTURE_IDLE}"
  if ! has_idle_marker "${CAPTURE_IDLE}"; then
    fail "missing idle marker in claude capture"
  fi
  log "observed stable claude lifecycle (running at t+${CLAUDE_WAIT_RUNNING_S}s, idle marker by t+${CLAUDE_WAIT_IDLE_S}s)"
}

main "$@"
