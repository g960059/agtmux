package integration

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

const (
	defaultTargetName = "local"
)

var notifyLinePattern = regexp.MustCompile(`(?m)^\s*notify\s*=`)

type InstallOptions struct {
	HomeDir          string
	BinDir           string
	AGTMUXBin        string
	TogglesExplicit  bool
	DryRun           bool
	InstallClaude    bool
	InstallCodex     bool
	InstallWrappers  bool
	ForceCodexNotify bool
}

type InstallResult struct {
	DryRun       bool     `json:"dry_run"`
	FilesWritten []string `json:"files_written,omitempty"`
	Backups      []string `json:"backups,omitempty"`
	Warnings     []string `json:"warnings,omitempty"`
}

func Install(opts InstallOptions) (InstallResult, error) {
	normalized, err := normalizeOptions(opts)
	if err != nil {
		return InstallResult{}, err
	}

	res := InstallResult{DryRun: normalized.DryRun}

	hookEmitPath := filepath.Join(normalized.BinDir, "agtmux-hook-emit")
	codexNotifyPath := filepath.Join(normalized.BinDir, "agtmux-codex-notify")
	codexWrapperPath := filepath.Join(normalized.BinDir, "agtmux-codex")
	claudeWrapperPath := filepath.Join(normalized.BinDir, "agtmux-claude")

	if normalized.InstallWrappers {
		if err := writeManagedFile(hookEmitPath, renderHookEmitScript(normalized.AGTMUXBin), 0o755, normalized.DryRun, &res); err != nil {
			return InstallResult{}, err
		}
		if err := writeManagedFile(codexNotifyPath, renderCodexNotifyScript(normalized.AGTMUXBin), 0o755, normalized.DryRun, &res); err != nil {
			return InstallResult{}, err
		}
		if err := writeManagedFile(codexWrapperPath, renderAgentWrapperScript(normalized.AGTMUXBin, "codex", "AGTMUX_CODEX_BIN", "codex"), 0o755, normalized.DryRun, &res); err != nil {
			return InstallResult{}, err
		}
		if err := writeManagedFile(claudeWrapperPath, renderAgentWrapperScript(normalized.AGTMUXBin, "claude", "AGTMUX_CLAUDE_BIN", "claude"), 0o755, normalized.DryRun, &res); err != nil {
			return InstallResult{}, err
		}
	}

	if normalized.InstallClaude {
		claudeSettingsPath := filepath.Join(normalized.HomeDir, ".claude", "settings.json")
		commands := map[string]string{
			"Notification": fmt.Sprintf("%s user-intervention-needed claude", hookEmitPath),
			"Stop":         fmt.Sprintf("%s task-finished claude", hookEmitPath),
			"SubagentStop": fmt.Sprintf("%s task-finished claude", hookEmitPath),
		}
		if err := mergeClaudeSettings(claudeSettingsPath, commands, normalized.DryRun, &res); err != nil {
			return InstallResult{}, err
		}
	}

	if normalized.InstallCodex {
		codexConfigPath := filepath.Join(normalized.HomeDir, ".codex", "config.toml")
		if err := mergeCodexConfig(codexConfigPath, codexNotifyPath, normalized.ForceCodexNotify, normalized.DryRun, &res); err != nil {
			return InstallResult{}, err
		}
	}

	return res, nil
}

func normalizeOptions(opts InstallOptions) (InstallOptions, error) {
	normalized := opts
	if strings.TrimSpace(normalized.HomeDir) == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return InstallOptions{}, fmt.Errorf("resolve home dir: %w", err)
		}
		normalized.HomeDir = home
	}
	if strings.TrimSpace(normalized.AGTMUXBin) == "" {
		normalized.AGTMUXBin = "agtmux"
	}
	if strings.TrimSpace(normalized.BinDir) == "" {
		normalized.BinDir = filepath.Join(normalized.HomeDir, ".local", "share", "agtmux", "bin")
	}
	if !normalized.TogglesExplicit &&
		!normalized.InstallClaude &&
		!normalized.InstallCodex &&
		!normalized.InstallWrappers {
		// Default to all when no explicit toggles are set.
		normalized.InstallClaude = true
		normalized.InstallCodex = true
		normalized.InstallWrappers = true
	}
	return normalized, nil
}

func mergeClaudeSettings(path string, commands map[string]string, dryRun bool, res *InstallResult) error {
	raw, err := readOptional(path)
	if err != nil {
		return err
	}

	updated, changed, err := applyClaudeCommands(raw, commands)
	if err != nil {
		return fmt.Errorf("merge claude settings: %w", err)
	}
	if !changed {
		return nil
	}
	return writeManagedFile(path, string(updated), 0o600, dryRun, res)
}

func applyClaudeCommands(raw []byte, commands map[string]string) ([]byte, bool, error) {
	var root map[string]any
	trimmed := strings.TrimSpace(string(raw))
	if trimmed == "" {
		root = map[string]any{}
	} else if err := json.Unmarshal(raw, &root); err != nil {
		return nil, false, fmt.Errorf("invalid JSON")
	}

	hooks := map[string]any{}
	if existing, ok := root["hooks"]; ok {
		asMap, ok := existing.(map[string]any)
		if !ok {
			return nil, false, fmt.Errorf("hooks must be object")
		}
		hooks = asMap
	}

	changed := false
	for event, cmd := range commands {
		entryList, _ := hooks[event].([]any)
		if entryList == nil {
			entryList = []any{}
		}
		idx := findMatcherEntry(entryList, "*")
		if idx < 0 {
			entryList = append(entryList, map[string]any{
				"matcher": "*",
				"hooks":   []any{},
			})
			idx = len(entryList) - 1
			changed = true
		}
		entry, ok := entryList[idx].(map[string]any)
		if !ok {
			entry = map[string]any{"matcher": "*", "hooks": []any{}}
		}
		hookList, _ := entry["hooks"].([]any)
		if hookList == nil {
			hookList = []any{}
		}
		if !containsHookCommand(hookList, cmd) {
			hookList = append(hookList, map[string]any{
				"type":    "command",
				"command": cmd,
			})
			changed = true
		}
		entry["hooks"] = hookList
		entryList[idx] = entry
		hooks[event] = entryList
	}
	root["hooks"] = hooks

	out, err := json.MarshalIndent(root, "", "  ")
	if err != nil {
		return nil, false, fmt.Errorf("marshal claude settings: %w", err)
	}
	out = append(out, '\n')

	if !changed && bytes.Equal(bytes.TrimSpace(raw), bytes.TrimSpace(out)) {
		return out, false, nil
	}
	return out, true, nil
}

func findMatcherEntry(entries []any, matcher string) int {
	for i, v := range entries {
		m, ok := v.(map[string]any)
		if !ok {
			continue
		}
		if strings.TrimSpace(toString(m["matcher"])) == matcher {
			return i
		}
	}
	return -1
}

func containsHookCommand(hooks []any, command string) bool {
	for _, h := range hooks {
		m, ok := h.(map[string]any)
		if !ok {
			continue
		}
		if strings.TrimSpace(toString(m["command"])) == strings.TrimSpace(command) {
			return true
		}
	}
	return false
}

func mergeCodexConfig(path, codexNotifyPath string, force, dryRun bool, res *InstallResult) error {
	raw, err := readOptional(path)
	if err != nil {
		return err
	}
	updated, changed, warning := applyCodexNotifySetting(string(raw), codexNotifyPath, force)
	if warning != "" {
		res.Warnings = append(res.Warnings, warning)
	}
	if !changed {
		return nil
	}
	return writeManagedFile(path, updated, 0o600, dryRun, res)
}

func applyCodexNotifySetting(raw, codexNotifyPath string, force bool) (updated string, changed bool, warning string) {
	const begin = "# >>> agtmux codex notify >>>"
	const end = "# <<< agtmux codex notify <<<"
	escapedPath := strings.ReplaceAll(codexNotifyPath, `\`, `\\`)
	escapedPath = strings.ReplaceAll(escapedPath, `"`, `\"`)
	line := fmt.Sprintf(`notify = ["sh", "-lc", "%s \"$1\""]`, escapedPath)
	block := begin + "\n" + line + "\n" + end

	start := strings.Index(raw, begin)
	finish := strings.Index(raw, end)
	if start >= 0 && finish > start {
		finish += len(end)
		replaced := raw[:start] + block + raw[finish:]
		if replaced == raw {
			return raw, false, ""
		}
		return normalizeTrailingNewline(replaced), true, ""
	}

	if notifyLinePattern.MatchString(raw) {
		if !force {
			return raw, false, "codex notify already exists; skipped (use --force-codex-notify to replace)"
		}
		notifyStart, notifyEnd := findNotifyAssignmentRange(raw)
		if notifyStart < 0 || notifyEnd <= notifyStart {
			return raw, false, "codex notify exists but could not be safely replaced"
		}
		replaced := raw[:notifyStart] + block + raw[notifyEnd:]
		return normalizeTrailingNewline(replaced), true, ""
	}

	if strings.TrimSpace(raw) == "" {
		return block + "\n", true, ""
	}
	return normalizeTrailingNewline(raw) + "\n" + block + "\n", true, ""
}

func normalizeTrailingNewline(s string) string {
	return strings.TrimRight(s, "\n")
}

func findNotifyAssignmentRange(raw string) (int, int) {
	loc := notifyLinePattern.FindStringIndex(raw)
	if loc == nil {
		return -1, -1
	}
	start := loc[0]
	lineEnd := strings.IndexByte(raw[start:], '\n')
	if lineEnd < 0 {
		return start, len(raw)
	}
	end := start + lineEnd + 1

	line := raw[start:end]
	eq := strings.IndexByte(line, '=')
	if eq < 0 {
		return start, end
	}
	afterEq := strings.TrimSpace(line[eq+1:])
	if !strings.HasPrefix(afterEq, "[") || strings.Contains(afterEq, "]") {
		return start, end
	}

	depth := 1
	for i := end; i < len(raw); i++ {
		switch raw[i] {
		case '[':
			depth++
		case ']':
			depth--
			if depth == 0 {
				j := i + 1
				for j < len(raw) && raw[j] != '\n' {
					j++
				}
				if j < len(raw) {
					j++
				}
				return start, j
			}
		}
	}
	return start, end
}

func writeManagedFile(path, content string, perm os.FileMode, dryRun bool, res *InstallResult) error {
	existing, err := readOptional(path)
	if err != nil {
		return err
	}
	if bytes.Equal(existing, []byte(content)) {
		return nil
	}

	if dryRun {
		res.FilesWritten = append(res.FilesWritten, path)
		return nil
	}

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("mkdir %s: %w", filepath.Dir(path), err)
	}
	if len(existing) > 0 {
		backupPath := fmt.Sprintf("%s.bak.%d", path, time.Now().UTC().UnixNano())
		if err := os.WriteFile(backupPath, existing, 0o600); err != nil {
			return fmt.Errorf("write backup %s: %w", backupPath, err)
		}
		res.Backups = append(res.Backups, backupPath)
	}

	tmpPath := fmt.Sprintf("%s.tmp.%d", path, time.Now().UTC().UnixNano())
	if err := os.WriteFile(tmpPath, []byte(content), perm); err != nil {
		return fmt.Errorf("write temp file %s: %w", tmpPath, err)
	}
	if err := os.Rename(tmpPath, path); err != nil {
		_ = os.Remove(tmpPath)
		return fmt.Errorf("rename temp file %s: %w", path, err)
	}
	res.FilesWritten = append(res.FilesWritten, path)
	return nil
}

func readOptional(path string) ([]byte, error) {
	b, err := os.ReadFile(path)
	if err == nil {
		return b, nil
	}
	if os.IsNotExist(err) {
		return nil, nil
	}
	return nil, fmt.Errorf("read file %s: %w", path, err)
}

func toString(v any) string {
	s, _ := v.(string)
	return s
}

func renderHookEmitScript(agtmuxBin string) string {
	return fmt.Sprintf(`#!/bin/sh
set -u
EVENT_TYPE="${1:-}"
AGENT_TYPE="${2:-unknown}"
if [ -z "$EVENT_TYPE" ]; then
  exit 0
fi
PANE_ID="${TMUX_PANE:-}"
if [ -z "$PANE_ID" ]; then
  exit 0
fi
AGTMUX_BIN="${AGTMUX_BIN:-%s}"
TARGET="${AGTMUX_TARGET:-%s}"
if [ -t 0 ]; then
  "$AGTMUX_BIN" event emit --target "$TARGET" --pane "$PANE_ID" --agent "$AGENT_TYPE" --source hook --type "$EVENT_TYPE" >/dev/null 2>&1 || true
else
  PAYLOAD="$(cat)"
  "$AGTMUX_BIN" event emit --target "$TARGET" --pane "$PANE_ID" --agent "$AGENT_TYPE" --source hook --type "$EVENT_TYPE" --payload "$PAYLOAD" >/dev/null 2>&1 || true
fi
exit 0
`, agtmuxBin, defaultTargetName)
}

func renderCodexNotifyScript(agtmuxBin string) string {
	return fmt.Sprintf(`#!/bin/sh
set -u
PANE_ID="${TMUX_PANE:-}"
if [ -z "$PANE_ID" ]; then
  exit 0
fi
AGTMUX_BIN="${AGTMUX_BIN:-%s}"
TARGET="${AGTMUX_TARGET:-%s}"
PAYLOAD="${1:-}"
EVENT_TYPE="agent-turn-complete"
if [ -n "$PAYLOAD" ]; then
  PAYLOAD_LOWER="$(printf '%%s' "$PAYLOAD" | tr '[:upper:]' '[:lower:]')"
  case "$PAYLOAD_LOWER" in
    *approval-requested*|*approval-needed*|*approval\ required*|*needs\ approval*|*awaiting\ approval*)
      EVENT_TYPE="approval-requested"
      ;;
    *input-requested*|*input-needed*|*user-intervention-needed*|*waiting\ for\ input*|*awaiting\ input*|*prompt-user*)
      EVENT_TYPE="input-requested"
      ;;
    *agent-turn-complete*|*agent-turn-finished*|*turn-finished*|*task\ completed*)
      EVENT_TYPE="agent-turn-complete"
      ;;
    *runtime-error*|*\"status\":\"error\"*|*\"status\":\"failed\"*|*\"result\":\"error\"*|*\"result\":\"failed\"*|*exception*|*panic*|*runtime\ error*)
      EVENT_TYPE="runtime-error"
      ;;
  esac
  "$AGTMUX_BIN" event emit --target "$TARGET" --pane "$PANE_ID" --agent codex --source notify --type "$EVENT_TYPE" --payload "$PAYLOAD" >/dev/null 2>&1 || true
else
  "$AGTMUX_BIN" event emit --target "$TARGET" --pane "$PANE_ID" --agent codex --source notify --type "$EVENT_TYPE" >/dev/null 2>&1 || true
fi
exit 0
`, agtmuxBin, defaultTargetName)
}

func renderAgentWrapperScript(agtmuxBin, agentType, binaryEnvKey, binaryDefault string) string {
	return fmt.Sprintf(`#!/bin/sh
set -u
AGTMUX_BIN="${AGTMUX_BIN:-%s}"
REAL_BIN="${%s:-%s}"
TARGET="${AGTMUX_TARGET:-%s}"
PANE_ID="${TMUX_PANE:-}"
if [ -n "$PANE_ID" ]; then
  "$AGTMUX_BIN" event emit --target "$TARGET" --pane "$PANE_ID" --agent %s --source wrapper --type wrapper-start >/dev/null 2>&1 || true
fi
"$REAL_BIN" "$@"
RC=$?
if [ -n "$PANE_ID" ]; then
  EVENT_TYPE="wrapper-exit"
  if [ "$RC" -ne 0 ]; then
    EVENT_TYPE="wrapper-error"
  fi
  "$AGTMUX_BIN" event emit --target "$TARGET" --pane "$PANE_ID" --agent %s --source wrapper --type "$EVENT_TYPE" --payload "exit_code=$RC" >/dev/null 2>&1 || true
fi
exit "$RC"
`, agtmuxBin, binaryEnvKey, binaryDefault, defaultTargetName, agentType, agentType)
}
