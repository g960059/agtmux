# Daemon API Specification

## Transport

- **CLI clients**: Unix domain socket (`/tmp/agtmux/agtmuxd.sock`)
- **Desktop/mobile clients**: WebSocket (`ws://localhost:PORT` or `wss://...`)

## Protocol

JSON-RPC 2.0 style over newline-delimited JSON.

### Request/Response

```json
// Request
{"id": 1, "method": "list_panes", "params": {}}

// Response
{"id": 1, "result": {
  "panes": [
    {
      "pane_id": "%1",
      "session_name": "main",
      "window_id": "@0",
      "pane_title": "claude-code",
      "current_cmd": "claude",
      "provider": "claude",
      "provider_confidence": 1.0,
      "activity_state": "running",
      "activity_confidence": 0.86,
      "activity_source": "poller",
      "attention_state": "none",
      "attention_reason": "",
      "attention_since": null,
      "presence": "managed",
      "updated_at": "2026-02-22T10:00:00Z"
    }
  ]
}}
```

### Subscription (Server → Client push)

クライアントは必要な情報だけを受け取るために filter を指定できる。

```json
// Subscribe (full — TUI 向け)
{"id": 2, "method": "subscribe", "params": {
  "events": ["state", "topology"]
}}

// Subscribe (filtered — 特定 pane のみ)
{"id": 2, "method": "subscribe", "params": {
  "events": ["state"],
  "filter": {
    "pane_ids": ["%1", "%2"]
  }
}}

// Subscribe (attention のみ — notification consumer 向け)
{"id": 2, "method": "subscribe", "params": {
  "events": ["state"],
  "filter": {
    "attention_only": true
  }
}}
```

#### Filter Options

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `pane_ids` | `string[]` | all | 指定 pane のみ |
| `attention_only` | `bool` | false | attention 変化のみ通知 |
| `min_confidence` | `float` | 0.0 | 低 confidence の変化を除外 |

#### Notifications

```json
// State change notification
{"method": "state_changed", "params": {
  "pane_id": "%1",
  "activity_state": "waiting_approval",
  "activity_confidence": 0.96,
  "activity_source": "hook",
  "attention_state": "action_required_approval",
  "attention_since": "2026-02-22T10:00:05Z"
}}

// Topology change notification
{"method": "pane_added", "params": {"pane_id": "%5", "session_name": "debug"}}
{"method": "pane_removed", "params": {"pane_id": "%3"}}
```

### Summary Subscription (tmux-status 向け)

集計値だけを受け取る軽量な subscription。tmux-status のように個別 pane の詳細が不要なクライアント向け。

```json
// Subscribe
{"id": 3, "method": "subscribe_summary", "params": {}}

// Summary notification (state が変わるたびに push)
{"method": "summary", "params": {
  "counts": {
    "running": 2,
    "waiting_input": 0,
    "waiting_approval": 1,
    "idle": 3,
    "error": 0,
    "unknown": 1
  },
  "attention_count": 1,
  "total_managed": 6,
  "total_unmanaged": 2
}}
```

### Terminal Output (binary)

WebSocket binary frame のみ。Unix socket では使わない（CLI は terminal output 不要）。

```
[1 byte: pane_alias_len][N bytes: pane_alias][remaining: raw terminal bytes]
```

### Commands

```json
{"id": 3, "method": "write_input", "params": {"pane_id": "%1", "data": "ls\n"}}
{"id": 4, "method": "resize_pane", "params": {"pane_id": "%1", "cols": 120, "rows": 40}}
```

## Implementation Location

| Module | Crate |
|--------|-------|
| Unix socket + WebSocket server | `agtmux-daemon/src/server.rs` |
| Orchestrator (source → engine → broadcast) | `agtmux-daemon/src/orchestrator.rs` |
| SQLite persistence | `agtmux-daemon/src/store.rs` |
| Source implementations (hook, api, file, poller) | `agtmux-daemon/src/sources/` |
