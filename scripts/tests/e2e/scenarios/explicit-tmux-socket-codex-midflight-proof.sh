#!/usr/bin/env bash
# scenarios/explicit-tmux-socket-codex-midflight-proof.sh
#
# Repo-owned proof for the post-launch Codex managed-surfacing question:
# on an exact tmux socket under an app-like daemon env, capture the same pane
# mid-flight (5-10s into a long-running Codex task) and after completion across:
#   - tmux exact-socket inventory
#   - daemon list_panes_snapshot
#   - daemon ui.bootstrap.v3
#
# Expected result:
#   - mid-flight: managed/provider=codex truth exists for the exact pane
#   - after completion: the pane may demote back to unmanaged shell truth

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../harness/common.sh"
source "$SCRIPT_DIR/../providers/codex/adapter.sh"

set -euo pipefail

TMUX_BIN="$(resolve_tmux_bin)"

if [[ "${AGTMUX_BIN:-}" != /* ]]; then
    REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || pwd)"
    if [ ! -x "$REPO_ROOT/target/debug/agtmux" ] && [ ! -x "$REPO_ROOT/target/release/agtmux" ]; then
        cargo build -p agtmux --quiet >/dev/null 2>&1
    fi
    if [ -x "$REPO_ROOT/target/debug/agtmux" ]; then
        AGTMUX_BIN="$REPO_ROOT/target/debug/agtmux"
    elif [ -x "$REPO_ROOT/target/release/agtmux" ]; then
        AGTMUX_BIN="$REPO_ROOT/target/release/agtmux"
    fi
fi
[ -x "$AGTMUX_BIN" ] || fail "agtmux binary must resolve to an absolute path"

SESSION="e2e-explicit-codex-midflight-$$"
WORKDIR="/tmp/agtmux-e2e-explicit-codex-midflight-$$"
SOCKET="$WORKDIR/agtmuxd.sock"
TMUX_SOCKET_PATH="$WORKDIR/tmux.sock"
LOG_PATH="$WORKDIR/daemon.log"
MIDFLIGHT_TMUX_PATH="$WORKDIR/midflight-tmux.txt"
MIDFLIGHT_LIST_PATH="$WORKDIR/midflight-list-panes-snapshot.json"
MIDFLIGHT_BOOTSTRAP_PATH="$WORKDIR/midflight-ui-bootstrap-v3.json"
FINAL_TMUX_PATH="$WORKDIR/final-tmux.txt"
FINAL_LIST_PATH="$WORKDIR/final-list-panes-snapshot.json"
FINAL_BOOTSTRAP_PATH="$WORKDIR/final-ui-bootstrap-v3.json"
PANE_ID=""
DAEMON_PID=""

echo "=== explicit-tmux-socket-codex-midflight-proof.sh ==="

tmux() {
    env -u TMUX -u TMUX_PANE "$TMUX_BIN" -S "$TMUX_SOCKET_PATH" "$@"
}

cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    tmux kill-server 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

dump_repro_context() {
    echo "--- tmux socket: $TMUX_SOCKET_PATH ---" >&2
    ls -l "$TMUX_SOCKET_PATH" >&2 || true
    echo "--- tmux panes ---" >&2
    tmux list-panes -a -F '#{session_name}|#{window_id}|#{pane_id}|#{pane_pid}|#{pane_current_command}' 2>&1 >&2 || true
    if [ -n "$PANE_ID" ]; then
        echo "--- pane capture ($PANE_ID) ---" >&2
        tmux capture-pane -p -t "$PANE_ID" 2>&1 >&2 || true
    fi
    echo "--- agtmux json ---" >&2
    "$AGTMUX_BIN" --socket-path "$SOCKET" json 2>&1 >&2 || true
    echo "--- midflight list_panes_snapshot ---" >&2
    cat "$MIDFLIGHT_LIST_PATH" 2>/dev/null >&2 || true
    echo "--- midflight ui.bootstrap.v3 ---" >&2
    cat "$MIDFLIGHT_BOOTSTRAP_PATH" 2>/dev/null >&2 || true
    echo "--- final list_panes_snapshot ---" >&2
    cat "$FINAL_LIST_PATH" 2>/dev/null >&2 || true
    echo "--- final ui.bootstrap.v3 ---" >&2
    cat "$FINAL_BOOTSTRAP_PATH" 2>/dev/null >&2 || true
    echo "--- daemon log ---" >&2
    cat "$LOG_PATH" >&2 || true
}

capture_probe_triplet() {
    local phase="$1" tmux_path="$2" list_path="$3" bootstrap_path="$4"

    tmux list-panes -t "$SESSION:main" -F '#{session_name}|#{window_id}|#{pane_id}|#{pane_pid}|#{pane_current_command}' \
        >"$tmux_path"
    daemon_rpc "$SOCKET" "list_panes_snapshot" >"$list_path"
    daemon_rpc "$SOCKET" "ui.bootstrap.v3" >"$bootstrap_path"

    local tmux_line snapshot_meta snapshot_row bootstrap_row
    tmux_line="$(cat "$tmux_path")"
    snapshot_meta="$(jq -c '.metadata' "$list_path" 2>/dev/null || echo 'null')"
    snapshot_row="$(jq -c --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p)' "$list_path" 2>/dev/null || echo 'null')"
    bootstrap_row="$(jq -c --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p)' "$bootstrap_path" 2>/dev/null || echo 'null')"

    log "$phase tmux=$tmux_line"
    log "$phase list_meta=$snapshot_meta"
    log "$phase list_row=$snapshot_row"
    log "$phase bootstrap_row=$bootstrap_row"
}

mkdir -p "$WORKDIR"
rm -f "$TMUX_SOCKET_PATH"

log "starting tmux session on explicit socket before daemon launch"
tmux new-session -d -s "$SESSION" -n main 'zsh -l' 2>/dev/null

for _ in $(seq 1 20); do
    PANE_ID="$(tmux list-panes -t "$SESSION:main" -F '#{pane_id}' 2>/dev/null | head -1 || true)"
    [ -n "$PANE_ID" ] && break
    sleep 0.5
done
[ -n "$PANE_ID" ] || {
    dump_repro_context
    fail "could not resolve pane_id for explicit tmux socket codex mid-flight proof"
}

APP_PATH="${APP_PATH:-/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin}"
APP_HOME="${HOME}"
APP_USER="${USER:-$(id -un)}"
APP_LOGNAME="${LOGNAME:-$APP_USER}"
APP_XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$APP_HOME/.config}"
APP_CODEX_HOME="${CODEX_HOME:-$APP_HOME/.codex}"

log "launching daemon with explicit --tmux-socket under app-like normalized env"
env -i \
    HOME="$APP_HOME" \
    USER="$APP_USER" \
    LOGNAME="$APP_LOGNAME" \
    XDG_CONFIG_HOME="$APP_XDG_CONFIG_HOME" \
    CODEX_HOME="$APP_CODEX_HOME" \
    PATH="$APP_PATH" \
    TMUX_BIN="$TMUX_BIN" \
    "$AGTMUX_BIN" --socket-path "$SOCKET" daemon --tmux-socket "$TMUX_SOCKET_PATH" \
    >"$LOG_PATH" 2>&1 &
DAEMON_PID=$!

wait_for_socket "$SOCKET" 15
wait_for_agtmux_state "$SOCKET" "$PANE_ID" "current_cmd" "zsh" 20 || {
    dump_repro_context
    fail "daemon never inventoried initial zsh pane $PANE_ID on explicit tmux socket"
}

TASK="Run exactly one bash command and do not run any additional commands. Wait 20 seconds by using sleep 20. bash -lc 'sleep 20; printf \"wait_result=managed\\n\"'. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=managed"

log "launching codex from plain zsh pane $PANE_ID"
launch_epoch="$(date +%s)"
launch_provider "$PANE_ID" "$WORKDIR" "$TASK"
wait_until_provider_running "$PANE_ID" 20 || log "WARN: provider-side running check timed out"

midflight_found=0
while true; do
    now_epoch="$(date +%s)"
    elapsed="$((now_epoch - launch_epoch))"

    if [ "$elapsed" -lt 5 ]; then
        sleep 1
        continue
    fi
    if [ "$elapsed" -gt 10 ]; then
        break
    fi

    capture_probe_triplet "midflight@${elapsed}s" \
        "$MIDFLIGHT_TMUX_PATH" "$MIDFLIGHT_LIST_PATH" "$MIDFLIGHT_BOOTSTRAP_PATH"

    midflight_snapshot_presence="$(jq -r --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p) | .presence // "null"' "$MIDFLIGHT_LIST_PATH" 2>/dev/null || echo "null")"
    midflight_snapshot_provider="$(jq -r --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p) | .provider // "null"' "$MIDFLIGHT_LIST_PATH" 2>/dev/null || echo "null")"
    midflight_bootstrap_presence="$(jq -r --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p) | .presence // "null"' "$MIDFLIGHT_BOOTSTRAP_PATH" 2>/dev/null || echo "null")"
    midflight_bootstrap_provider="$(jq -r --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p) | .provider // "null"' "$MIDFLIGHT_BOOTSTRAP_PATH" 2>/dev/null || echo "null")"

    if [ "$midflight_snapshot_presence" = "managed" ] \
        && [ "$midflight_snapshot_provider" = "codex" ] \
        && [ "$midflight_bootstrap_presence" = "managed" ] \
        && [ "$midflight_bootstrap_provider" = "codex" ]; then
        midflight_found=1
        break
    fi

    sleep 1
done

[ "$midflight_found" -eq 1 ] || {
    dump_repro_context
    fail "mid-flight exact-socket proof never surfaced managed/provider=codex in list_panes_snapshot + ui.bootstrap.v3 within 5-10s"
}

completion_mode="$(wait_for_completion_or_shell_demotion "$SOCKET" "$PANE_ID" 45)" || {
    dump_repro_context
    fail "pane never reached managed completion or shell demotion after codex run"
}

capture_probe_triplet "final-$completion_mode" \
    "$FINAL_TMUX_PATH" "$FINAL_LIST_PATH" "$FINAL_BOOTSTRAP_PATH"

final_bootstrap_presence="$(jq -r --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p) | .presence // "null"' "$FINAL_BOOTSTRAP_PATH" 2>/dev/null || echo "null")"
final_bootstrap_provider="$(jq -r --arg p "$PANE_ID" '.panes[]? | select(.pane_id==$p) | .provider // "null"' "$FINAL_BOOTSTRAP_PATH" 2>/dev/null || echo "null")"

if [ "$completion_mode" = "unmanaged" ]; then
    assert_eq "final bootstrap presence after shell demotion" "unmanaged" "$final_bootstrap_presence"
    assert_eq "final bootstrap provider after shell demotion" "null" "$final_bootstrap_provider"
else
    assert_eq "final bootstrap presence after managed completion" "managed" "$final_bootstrap_presence"
    assert_eq "final bootstrap provider after managed completion" "codex" "$final_bootstrap_provider"
fi

pass "exact-socket mid-flight proof saw managed codex truth before completion; final state=$completion_mode"
echo "=== explicit-tmux-socket-codex-midflight-proof.sh PASS ==="
