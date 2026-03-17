# Product Architecture

## Runtime Boundary

- The shipped entrypoint is a single `agtmux` binary that wires runtime, daemon, gateway, tmux IO, and source adapters in process.
- Pure domain logic lives in `agtmux-core-v5`; tmux and provider integrations stay at the IO edges.

## Components

- `C-002` `agtmux-daemon-v5`: canonical pane/session projection and client API
- `C-003` `agtmux-gateway`: source aggregation and cursor tracking
- `C-004` `agtmux-source-codex-appserver`: Codex app-server adapter kept for maintained integrations in this repo
- `C-005` `agtmux-source-claude-hooks`: deterministic Claude hook ingestion
- `C-006` `agtmux-source-poller`: heuristic fallback detection
- `C-007` `agtmux-source-claude-jsonl`: deterministic Claude transcript ingestion
- `C-015` `agtmux-tmux-v5`: tmux and process inspection boundary
- `C-016` `agtmux-runtime`: CLI, poll loop, UDS server, and runtime wiring

## Core Contracts

- Fresh deterministic evidence suppresses heuristic state for the same pane.
- Fallback remains available when deterministic sources go stale or down.
- Pane identity is `pane_id + generation + birth_ts`.
- Startup order is source -> gateway -> daemon -> UI when the runtime is supervising dependencies.
- Durable cross-cutting behavior belongs in ADRs, tests, and operator runbooks.
