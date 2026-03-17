# Review Pack: agtmux-term semantic truth ownership handover (T-XTERM-A4)

Date: 2026-03-08
Owner: agtmux (daemon / producer side)
Counterpart: agtmux-term (consumer / sidebar side)
Policy: semantic truth is producer-owned; consumer mirrors boundary truth only

## Decision (locked)

Cross-repo live E2E ownership is split by oracle, not by scenario name.

- `agtmux` owns producer-side semantic truth for real CLI scenarios.
- `agtmux-term` owns thin daemon-to-sidebar canaries.
- Same provider scenario can exist in both repos, but pass/fail oracle must differ.

This handover includes no daemon code changes; it fixes ownership and validation boundaries.

## Responsibility boundary

| Repo | Owns | Must not own |
|---|---|---|
| `agtmux` | Semantic truth generation and producer-side oracle (`provider`, `presence`, `activity_state`, `conversation_title`, no-bleed) | Exact-row SwiftUI rendering behavior |
| `agtmux-term` | Consumer truth: exact row receives daemon payload, no bleed to sibling rows, sidebar rendering continuity | Reimplementation of full producer semantic matrix |

## Daemon-owned scenario matrix (source of truth)

| Semantic target | Producer-side oracle (agtmux) | Primary scenarios / scripts |
|---|---|---|
| `provider` selection | winning provider in daemon JSON | `scripts/tests/e2e/scenarios/provider-switch.sh`, `scripts/tests/e2e/scenarios/source-competition.sh` |
| `presence=managed` | pane is managed with deterministic evidence | `scripts/tests/e2e/scenarios/single-agent-lifecycle.sh`, `scripts/tests/e2e/scenarios/multi-agent-same-session.sh`, `scripts/tests/e2e/scenarios/same-cwd-multi-pane.sh` |
| `activity_state=running` | running emitted while provider is active | `scripts/tests/e2e/scenarios/single-agent-lifecycle.sh`, `scripts/tests/e2e/scenarios/codex-semantic-states.sh`, `scripts/tests/e2e/scenarios/codex-tool-execution.sh` |
| completion state (`idle` or `waiting_input`) | completion transition exposed in daemon JSON | `scripts/tests/e2e/scenarios/single-agent-lifecycle.sh`, `scripts/tests/e2e/scenarios/codex-semantic-states.sh`, `scripts/tests/e2e/scenarios/codex-session-rotation.sh` |
| `waiting_input` | explicit waiting input state exposure | `scripts/tests/e2e/scenarios/codex-semantic-states.sh`, `scripts/tests/e2e/scenarios/codex-tool-execution.sh`, `scripts/tests/e2e/scenarios/codex-approval-flow.sh`, `scripts/tests/e2e/contract/test-claude-approval.sh` |
| `waiting_approval` | explicit approval-waiting state exposure | `scripts/tests/e2e/scenarios/codex-approval-flow.sh`, `scripts/tests/e2e/contract/test-claude-approval.sh` |
| `conversation_title` | daemon title extraction and retention | `scripts/tests/e2e/scenarios/claude-title.sh`, `scripts/tests/e2e/scenarios/claude-summary.sh`, `scripts/tests/e2e/scenarios/claude-title-after-restart.sh`, `scripts/tests/e2e/scenarios/codex-title.sh` |
| no-bleed across sibling/same-session/same-CWD panes | pane A signals do not collapse pane B ownership/state | `scripts/tests/e2e/scenarios/multi-agent-same-session.sh`, `scripts/tests/e2e/scenarios/same-cwd-multi-pane.sh`, `scripts/tests/e2e/scenarios/provider-switch.sh` |

## Provider-specific live prompt guidance

The producer-side live canary should keep prompts deterministic and short-lived so state transitions are observable.

### Claude Sonnet 4.6

- Launch model: `claude-sonnet-4-6`
- Canonical command shape:
  - `claude --dangerously-skip-permissions --model claude-sonnet-4-6`
- Recommended waiting prompt (strict):
  - `Run exactly one bash command and do not run any additional commands. wait 60 seconds by using sleep 60. bash -lc 'sleep 60; printf "wait_result=%s\n" "<running|idle>"' Use the same command shape, replacing <running|idle> with the observed state after the wait. Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=<running|idle>`
- Script reference: `scripts/tests/test-source-claude.sh`

### Codex 5.4 medium

- Launch model/effort: `gpt-5.4-codex` + `model_reasoning_effort="medium"`
- Canonical command shape:
  - `codex exec --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check --json --model gpt-5.4-codex -c model_reasoning_effort="medium"`
- Recommended waiting prompt (strict):
  - `Run exactly one bash command and do not run any additional commands. wait 60 seconds by using sleep 60. bash -lc 'sleep 60; printf "wait_result=idle\n"' Do not simulate, infer, or guess. Output only one non-empty line. Required output format: wait_result=idle`
- Script reference: `scripts/tests/test-source-codex.sh` (`CODEX_MODEL` / `CODEX_EFFORT` override)

## Preflight (must be explicit before live runs)

Run `just preflight-online` and fail closed on any missing prerequisite:

1. tmux is available (`tmux -V`)
2. provider CLIs are present (`codex`, `claude`)
3. auth is ready (CLI auth status or API key fallback)
4. network/API path is reachable

## agtmux-term mirror scope (consumer canary only)

`agtmux-term` should mirror daemon payload truth, not rebuild producer semantics:

1. exact target row is `running` when daemon row is `running`
2. completion state on that row matches daemon truth
3. sibling rows do not inherit target-row provider/activity/title metadata
4. sidebar render path remains aligned with daemon row truth

## Tracking / gate relation

- Task: `docs/60_tasks.md` `T-XTERM-A4`
- This document satisfies: `docs-first handover exists for agtmux-term`
- Remaining cross-repo dependency: `T-XTERM-A3` compatibility handback before strict consumer smoke closure
