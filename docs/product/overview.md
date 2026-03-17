# Product Overview

## What

- `agtmux` watches tmux panes and surfaces which agent panes need attention.
- It combines deterministic provider signals with heuristic fallback from tmux and process inspection.

## Users

- Developers running Claude Code, Codex, Gemini, or similar agents in parallel tmux panes.
- Maintainers who need reliable, testable state projection for those panes.

## Product Rules

- Fresh deterministic evidence wins over heuristic evidence for the same pane.
- Heuristic polling stays active so monitoring survives source outages.
- `managed` / `unmanaged` describes agent-session presence, not evidence source.
- Pane identity is pane-first so tmux pane reuse does not corrupt state.

## Success Criteria

- Attention-relevant state changes surface within seconds.
- Deterministic outages degrade to fallback instead of dropping visibility.
- Durable behavior is enforced in code, tests, and ADRs rather than long-lived implementation specs.
