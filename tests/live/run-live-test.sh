#!/usr/bin/env bash
# AGTMUX live integration test orchestrator.
# Exit codes: 0 = pass, 1 = accuracy fail, 2 = infrastructure fail.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

# ── Defaults ─────────────────────────────────────────────────────────────────

HOLD_SEC=8
WINDOW_SEC=5
AGTMUX=""
REAL_FLAG=""

# ── Parse arguments ──────────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
    case "$1" in
        --real)       REAL_FLAG="--real"; shift ;;
        --hold-sec)   HOLD_SEC="$2"; shift 2 ;;
        --window-sec) WINDOW_SEC="$2"; shift 2 ;;
        --agtmux)     AGTMUX="$2"; shift 2 ;;
        *) log_error "unknown argument: $1"; exit 2 ;;
    esac
done

# ── Find agtmux binary ──────────────────────────────────────────────────────

if [ -z "$AGTMUX" ]; then
    if [ -x "$PROJECT_ROOT/target/release/agtmux" ]; then
        AGTMUX="$PROJECT_ROOT/target/release/agtmux"
    elif [ -x "$PROJECT_ROOT/target/debug/agtmux" ]; then
        AGTMUX="$PROJECT_ROOT/target/debug/agtmux"
    elif command -v agtmux >/dev/null 2>&1; then
        AGTMUX="$(command -v agtmux)"
    else
        log_error "agtmux binary not found"
        exit 2
    fi
fi

log_info "using agtmux: $AGTMUX"

# ── Preflight ────────────────────────────────────────────────────────────────

log_info "running preflight"
if ! "$AGTMUX" preflight $REAL_FLAG; then
    log_error "preflight failed"
    exit 2
fi

# ── Temp directory & isolation paths ─────────────────────────────────────────

TMPDIR_LIVE="$(mktemp -d /tmp/agtmux-live-XXXXXX)"
TMUX_SESSION="agtmux-live-test-$$"
SOCKET="$TMPDIR_LIVE/daemon-$$.sock"
HOOK_SOCKET="$TMPDIR_LIVE/hook-$$.sock"
DB_PATH="$TMPDIR_LIVE/state-$$.db"
RECORDING="$TMPDIR_LIVE/recording-$$.jsonl"
CLAUDE_TIMESTAMPS="$TMPDIR_LIVE/claude-timestamps.jsonl"
CODEX_TIMESTAMPS="$TMPDIR_LIVE/codex-timestamps.jsonl"
DAEMON_PID=""

trap cleanup EXIT

# ── Create tmux session with Claude pane ─────────────────────────────────────

log_info "creating tmux session: $TMUX_SESSION"
tmux new-session -d -s "$TMUX_SESSION" -x 120 -y 40
tmux select-pane -T "claude" -t "$TMUX_SESSION"

CLAUDE_PANE_ID="$(tmux display-message -p -t "$TMUX_SESSION" '#{pane_id}')"
log_info "claude pane: $CLAUDE_PANE_ID"

# ── Start daemon ─────────────────────────────────────────────────────────────

log_info "starting daemon (recording to $RECORDING)"
"$AGTMUX" daemon \
    --record "$RECORDING" \
    --socket "$SOCKET" \
    --hook-socket "$HOOK_SOCKET" \
    --db-path "$DB_PATH" \
    --poll-interval-ms 500 &
DAEMON_PID=$!

log_info "daemon pid: $DAEMON_PID"

# Wait for socket to appear (30s timeout).
log_info "waiting for daemon socket"
if ! wait_for_file "$SOCKET" 30; then
    log_error "daemon socket did not appear within 30s"
    exit 2
fi
log_info "daemon socket ready"

# ── Run mock-claude.sh in Claude pane ────────────────────────────────────────

log_info "sending mock-claude.sh to pane $CLAUDE_PANE_ID"
tmux send-keys -t "$TMUX_SESSION" \
    "HOLD_SEC=$HOLD_SEC TIMESTAMP_FILE=$CLAUDE_TIMESTAMPS PANE_ID=$CLAUDE_PANE_ID bash $SCRIPT_DIR/mock-claude.sh" Enter

# ── Create Codex window ──────────────────────────────────────────────────────

tmux new-window -t "$TMUX_SESSION"
tmux select-pane -T "codex" -t "$TMUX_SESSION"

CODEX_PANE_ID="$(tmux display-message -p -t "$TMUX_SESSION" '#{pane_id}')"
log_info "codex pane: $CODEX_PANE_ID"

log_info "sending mock-codex.sh to pane $CODEX_PANE_ID"
tmux send-keys -t "$TMUX_SESSION" \
    "HOLD_SEC=$HOLD_SEC TIMESTAMP_FILE=$CODEX_TIMESTAMPS PANE_ID=$CODEX_PANE_ID bash $SCRIPT_DIR/mock-codex.sh" Enter

# ── Wait for mock scripts to finish ──────────────────────────────────────────

WAIT_SEC=$(( 8 * HOLD_SEC + 10 ))
log_info "waiting up to ${WAIT_SEC}s for mock scripts to complete"
sleep "$WAIT_SEC"

# Verify MOCK_COMPLETE marker in both panes.
CLAUDE_CAPTURE="$(tmux capture-pane -p -t "$CLAUDE_PANE_ID" 2>/dev/null || true)"
CODEX_CAPTURE="$(tmux capture-pane -p -t "$CODEX_PANE_ID" 2>/dev/null || true)"

if ! echo "$CLAUDE_CAPTURE" | grep -q "MOCK_COMPLETE"; then
    log_error "MOCK_COMPLETE not found in claude pane"
    exit 2
fi
if ! echo "$CODEX_CAPTURE" | grep -q "MOCK_COMPLETE"; then
    log_error "MOCK_COMPLETE not found in codex pane"
    exit 2
fi
log_pass "both mock scripts completed"

# ── Stop daemon ──────────────────────────────────────────────────────────────

log_info "stopping daemon (pid $DAEMON_PID)"
kill "$DAEMON_PID" 2>/dev/null || true
wait "$DAEMON_PID" 2>/dev/null || true
DAEMON_PID=""

# Allow recorder to flush.
sleep 2

if [ ! -s "$RECORDING" ]; then
    log_error "recording file is empty or missing: $RECORDING"
    exit 2
fi
log_info "recording file: $(wc -l < "$RECORDING") lines"

# ── Auto-label ───────────────────────────────────────────────────────────────

LABELED="$TMPDIR_LIVE/labeled-$$.jsonl"

# Combine both timestamp files into a single expected-labels file.
EXPECTED="$TMPDIR_LIVE/expected-$$.jsonl"
cat "$CLAUDE_TIMESTAMPS" "$CODEX_TIMESTAMPS" > "$EXPECTED"

log_info "running auto-label (window=${WINDOW_SEC}s)"
"$AGTMUX" auto-label "$RECORDING" "$EXPECTED" \
    --window-sec "$WINDOW_SEC" \
    --output "$LABELED"

if [ ! -s "$LABELED" ]; then
    log_error "labeled file is empty or missing: $LABELED"
    exit 2
fi

# ── Accuracy ─────────────────────────────────────────────────────────────────

log_info "running accuracy check"
if "$AGTMUX" accuracy "$LABELED"; then
    log_pass "accuracy gates passed"
    exit 0
else
    log_fail "accuracy gates failed"
    exit 1
fi
