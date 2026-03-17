# ADR: MVP Single-Process Runtime

- Date: 2026-02-25
- Status: Accepted
- Context: v5 runtime integration — how to wire pure-logic crates into a running binary

## Decision

MVP uses a **single-process binary** (`agtmux`) that embeds all components in-process
via direct function calls. Multi-process extraction is deferred to Post-MVP.

## Alternatives Considered

### A) Single binary with in-process wiring (chosen)
- All 6 existing crates are pure state machines accepting typed data structures.
- `Gateway::ingest_source_response()` takes `PullEventsResponse` directly.
- `DaemonProjection::apply_events()` takes `Vec<SourceEventV2>` directly.
- No serialization/deserialization overhead.

### B) Multi-process per architecture diagram
- Supervisor starts 5 child processes communicating over 6 UDS socket pairs.
- Requires: 5+ separate binaries, process supervisor with health probing,
  JSON-RPC serialization layer, per-process startup ordering.
- Blocks first runnable CLI behind significant infrastructure work.

## Rationale

Option A reaches "CLI runs" with minimum effort because the existing crate APIs
already accept in-process data. The pure-logic crates remain unchanged regardless
of whether they are called in-process or across UDS. Extraction to multi-process
later requires only adding transport adapters — no logic crate changes.

## Consequences

- Defer: UDS trust boundary between components, process supervisor strict contract,
  per-source process health probing, per-component independent restart.
- Accept: single failure domain for MVP (one process crash takes down all components).
- Initial bootstrap is poller-only; deterministic source IO adapters added incrementally.
- 200ms/250ms poll intervals from the architecture docs apply to multi-process topology;
  single-process MVP uses a configurable interval (default 1000ms).

## Additional decisions (from review)

- **Cursor contract fix**: Sources must always return `Some(current_position)` for
  `next_cursor` even when caught up. Gateway always overwrites tracker cursor.
  Without this, caught-up sources return `None`, gateway skips cursor update,
  and the same events are re-delivered every poll tick.
- **Unmanaged pane tracking**: Poll loop emits synthetic events for non-agent panes
  so the daemon tracks all tmux panes (FR-009 compliance).
- **Memory compaction**: Poll loop compacts poller/gateway/daemon buffers after each
  daemon apply to prevent unbounded memory growth.
- **Socket security**: `/tmp/agtmux-$UID/` (mode 0700) + socket (mode 0600) from MVP.
- **Logging**: `tracing` + `tracing-subscriber` with `AGTMUX_LOG` env var.
