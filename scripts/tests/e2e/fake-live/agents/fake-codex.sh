#!/usr/bin/env bash
# fake-live/agents/fake-codex.sh — deterministic Codex exec-jsonl driver

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

PROFILE="${1:-slow_complete}"

case "$PROFILE" in
    slow_complete)
        printf '%s\n' '{"type":"thread.started","thread_id":"thr_fake"}'
        sleep_with_jitter 1
        printf '%s\n' '{"type":"turn.started","turn_id":"turn_fake"}'
        sleep_with_jitter 1
        printf '%s\n' '{"type":"item.started","item":{"id":"item_fake","type":"command_execution","status":"in_progress"}}'
        sleep_with_jitter 2
        printf '%s\n' '{"type":"item.completed","item":{"id":"item_fake","type":"command_execution","status":"completed","exit_code":0}}'
        sleep_with_jitter 1
        printf '%s\n' '{"type":"turn.completed","turn_id":"turn_fake"}'
        printf 'codex> done\n'
        ;;
    long_running)
        printf '%s\n' '{"type":"thread.started","thread_id":"thr_fake_long"}'
        sleep_with_jitter 1
        printf '%s\n' '{"type":"turn.started","turn_id":"turn_fake_long"}'
        sleep_with_jitter 1
        printf '%s\n' '{"type":"item.started","item":{"id":"item_fake_long","type":"command_execution","status":"in_progress"}}'
        printf 'Codex is still running\n'
        sleep_with_jitter 8
        printf '%s\n' '{"type":"item.completed","item":{"id":"item_fake_long","type":"command_execution","status":"completed","exit_code":0}}'
        sleep_with_jitter 1
        printf '%s\n' '{"type":"turn.completed","turn_id":"turn_fake_long"}'
        printf 'codex> complete\n'
        ;;
    *)
        echo "[fake-codex] unknown profile: $PROFILE" >&2
        exit 1
        ;;
esac
