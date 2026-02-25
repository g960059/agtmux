#!/usr/bin/env bash
set -euo pipefail

SESSION="agtmux-e2e-poller-$(date +%s)-$$"
WINDOW="e2e"
KEEP_TMUX_VALUE="${KEEP_TMUX:-0}"
KEEP_WORKDIR_VALUE="${KEEP_WORKDIR:-0}"
PANE_ID=""
PANE_PID=""
WORKDIR=""
WAIT60_RUNNING_S=40
WAIT60_IDLE_S=120

log() {
  echo "[poller-e2e] $*"
}

fail() {
  echo "[poller-e2e][error] $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

create_workspace() {
  WORKDIR="$(mktemp -d "/tmp/agtmux-e2e-poller-XXXXXX")" || fail "failed to create temp workspace"
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

pane_cmd() {
  tmux display-message -p -t "${PANE_ID}" "#{pane_current_command}"
}

capture_text() {
  tmux capture-pane -p -t "${PANE_ID}" -S -200
}

print_capture() {
  local label="$1"
  local text="$2"
  echo "===== POLLER CAPTURE: ${label} ====="
  printf '%s\n' "${text}"
  echo "===== END POLLER CAPTURE: ${label} ====="
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

main() {
  trap cleanup EXIT
  require_cmd tmux
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

  log "sending local baseline command from isolated workspace: sleep 60"
  send_line "sleep 60"

  log "observing around t+${WAIT60_RUNNING_S}s (v4 capture.wait60.running_s)"
  sleep "${WAIT60_RUNNING_S}"
  CMD_40="$(pane_cmd)"
  CAPTURE_40="$(capture_text)" || fail "capture-pane failed at t+${WAIT60_RUNNING_S}s"
  print_capture "t+${WAIT60_RUNNING_S}s (wait60.running_s cmd=${CMD_40})" "${CAPTURE_40}"
  if [ "${CMD_40}" != "sleep" ]; then
    fail "heuristic check failed: expected pane_current_command=sleep around t+${WAIT60_RUNNING_S}s, got '${CMD_40}'"
  fi

  log "observing around t+${WAIT60_IDLE_S}s (v4 capture.wait60.idle_s)"
  sleep "$((WAIT60_IDLE_S - WAIT60_RUNNING_S))"
  CMD_120="$(pane_cmd)"
  CAPTURE_120="$(capture_text)" || fail "capture-pane failed at t+${WAIT60_IDLE_S}s"
  print_capture "t+${WAIT60_IDLE_S}s (wait60.idle_s cmd=${CMD_120})" "${CAPTURE_120}"
  if [ "${CMD_120}" = "sleep" ]; then
    fail "heuristic check failed: sleep still running around t+${WAIT60_IDLE_S}s"
  fi

  log "heuristic progression OK (running at ~40s, no longer running at ~120s)"
  log "TODO assertion: tighten progression validation when source server emits poller state traces"
}

main "$@"
