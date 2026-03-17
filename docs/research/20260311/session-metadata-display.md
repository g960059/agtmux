# Research: Session Metadata Display for agtmux-term Sidebar

Date: 2026-03-11
Context: User request to display conversation title, last user input, updatedAt in sidebar for agent panes

---

## 1. What's Already Available (No Code Change Needed)

### agtmux daemon — already serialized in sync-v3 JSON

| Field | Key | Source | Status |
|-------|-----|--------|--------|
| conversation_title | `conversation_title` | `DaemonState.conversation_titles[session_key]` | ✅ Already in API |
| last activity time | `updated_at` | `SyncV3PaneSnapshot.updated_at` | ✅ Already in API |
| age in seconds | `age_secs` | computed from updated_at | ✅ Already in API |
| session_key | `metadata_session_key` (partial) | pane.session_key | ✅ In API |

### agtmux-term — already in AgtmuxPane model

- `conversationTitle: String?` (CoreModels.swift:168)
- `updatedAt: Date?` (CoreModels.swift)
- `ageSecs: Int?` (CoreModels.swift)
- **Already displayed**: via `primaryLabel` → `conversationTitle ?? provider.rawValue ?? paneId`

### conversation_title population (DaemonState.conversation_titles)

Priority chain (highest first):
1. `custom-title` event in Claude JSONL (explicit user action)
2. `summary` from SessionFileWatcher (AI-generated, real-time)
3. `summary` from sessions-index.json (historical fallback)
4. `firstPrompt` from sessions-index.json (historical)
5. `last_first_prompt` from watcher (first user message text)

---

## 2. What's NOT Yet Available (Requires New Code)

### Missing fields in sync-v3 API response (server.rs)

| Field | Description | Data Source |
|-------|-------------|-------------|
| `session_summary` / subtitle | Short description of what user is doing | `watcher.last_summary()` or `first_prompt` |
| `session_id` | UUID of the JSONL session | From watcher / sessions-index entry |
| `created_at` | Session start time | From JSONL first line `timestamp` |
| `message_count` | How many exchanges | sessions-index.json `messageCount` |
| `git_branch` | Active git branch during session | JSONL header, sessions-index entry |

### Missing in AgtmuxPane model (CoreModels.swift)

- No `sessionSummary` / subtitle field
- No `sessionId` (the UUID, not tmux session ID)
- No `createdAt` for session start

---

## 3. Claude JSONL Session Format

**sessions-index.json entry** (real data):
```json
{
  "sessionId": "f1fca7e9-7460-4fdc-b240-d2cb42ded709",
  "summary": "Cloud Run本番環境エラー調査、Supabase環境変数設定修正",
  "firstPrompt": "cloud runの",
  "modified": "2026-02-01T08:04:13.254Z",
  "created": "2026-02-01T07:54:48.222Z",
  "messageCount": 11,
  "gitBranch": "main"
}
```

**JSONL line types for title/summary**:
- `type=custom-title` → `customTitle` field (user-set)
- `type=summary` → `summary` field (AI-generated)
- First `type=user` → `message.content[0].text` (fallback)

---

## 4. Codex JSONL Session Format

**session_meta payload** (real data):
```json
{
  "id": "019cb4bd-e75c-7002-ab83-72a997fb86dd",
  "timestamp": "2026-03-03T17:27:50.396Z",
  "cwd": "/Users/.../agtmux",
  "originator": "codex_exec",
  "cli_version": "0.106.0",
  "source": "exec",
  "git": { "branch": "main", ... }
}
```

**Key difference**: Codex has NO explicit title field.
Title = first `response_item` with `role="user"` → `content[0].text`.
agtmux already extracts this as `last_first_prompt`.

**turn_context** has per-turn `summary` field (auto-generated placeholder).

---

## 5. How Similar Tools Display Session Lists

### `codex /resume` (CLI picker)
- Lists sessions filtered by CWD by default
- Sort by file mtime (most recent first)
- Display: session ID + first user message snippet + relative time
- Interactive fzf-style picker

### Claude web app (projects/history)
- Title (custom-title > summary > first prompt)
- Relative timestamp ("2 hours ago")
- No inline subtitle (click to expand)

### Claude Code CLI (`/resume`)
- Shows session list with: title + created time
- Derived from sessions-index.json

### agtmux `ls` command (current)
```
session: main
  ● claude  [det]  Implement OAuth login   ~/repo   2m
  ● codex   [det]  Fix failing tests       ~/repo   5m
```
- Shows: title (conversation_title) + path + age

---

## 6. Current agtmux-term Sidebar Row Layout

```
[state icon] [conversation_title or provider]  ... [provider icon] [freshness]
```

- Font: 13pt rounded, regular (selected: semibold)
- 1 line, truncate tail
- `paneDisplayTitle(for:)` → `conversationTitle ?? provider ?? paneId`
- Freshness shown for idle/waiting/error states only

**Gap**: No subtitle (summary/firstPrompt), no visual distinction between "no title yet" and "title is provider name"

---

## 7. Proposed Enhancement Options

### Option A — Minimal: Improve existing 1-line display
- Show `updatedAt` more prominently (currently behind `ageSecs`)
- Differentiate "has real title" vs "placeholder provider name"
- No API changes needed

### Option B — 2-line row with subtitle
```
[●] Implement OAuth login         [claude] [2m]
    /repo main • "add refresh token support..."
```
- Line 1: conversation_title (bold)
- Line 2: subtitle = summary/firstPrompt snippet (gray, 11pt)
- Requires: new `session_subtitle` field in API + model

### Option C — Rich metadata with session browser
- 2-line row + popover/sheet showing full session details
- session_id, created_at, message_count, git_branch
- Requires: new fields in API

---

## 8. Key Implementation Constraints

1. **conversation_title is already working** — T-135a/b/c completed
2. **New subtitle field** — needs: server.rs + AgtmuxPane + SidebarView changes
3. **Data availability**:
   - `summary` is available in `watcher.last_summary()` for active sessions
   - `first_prompt` is available in `watcher.last_first_prompt()`
   - Both are already in `DaemonState.conversation_titles` pipeline context
4. **Codex subtitle** = `last_first_prompt` (no summary until turn_context processing added)
5. **Row height constraint**: Current sidebar uses fixed row height — 2-line requires layout change
