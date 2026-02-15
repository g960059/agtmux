package integration

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

type DoctorOptions struct {
	HomeDir string
	BinDir  string
}

type DoctorCheck struct {
	Name    string `json:"name"`
	Status  string `json:"status"` // pass | warn | fail
	Message string `json:"message"`
	Path    string `json:"path,omitempty"`
}

type DoctorResult struct {
	OK       bool          `json:"ok"`
	Checks   []DoctorCheck `json:"checks"`
	Warnings []string      `json:"warnings,omitempty"`
}

func Doctor(opts DoctorOptions) (DoctorResult, error) {
	normalized, err := normalizeOptions(InstallOptions{
		HomeDir: opts.HomeDir,
		BinDir:  opts.BinDir,
	})
	if err != nil {
		return DoctorResult{}, err
	}
	hookEmitPath := filepath.Join(normalized.BinDir, "agtmux-hook-emit")
	codexNotifyPath := filepath.Join(normalized.BinDir, "agtmux-codex-notify")
	codexWrapperPath := filepath.Join(normalized.BinDir, "agtmux-codex")
	claudeWrapperPath := filepath.Join(normalized.BinDir, "agtmux-claude")

	out := DoctorResult{OK: true}
	add := func(c DoctorCheck) {
		out.Checks = append(out.Checks, c)
		if c.Status == "warn" {
			out.Warnings = append(out.Warnings, fmt.Sprintf("%s: %s", c.Name, c.Message))
		}
		if c.Status == "fail" {
			out.OK = false
		}
	}

	add(checkManagedScript("hook_emit", hookEmitPath))
	add(checkManagedScript("codex_notify", codexNotifyPath))
	add(checkManagedScript("codex_wrapper", codexWrapperPath))
	add(checkManagedScript("claude_wrapper", claudeWrapperPath))

	claudeSettingsPath := filepath.Join(normalized.HomeDir, ".claude", "settings.json")
	claudeCheck, err := checkClaudeSettings(claudeSettingsPath, hookEmitPath)
	if err != nil {
		return DoctorResult{}, err
	}
	add(claudeCheck)

	codexConfigPath := filepath.Join(normalized.HomeDir, ".codex", "config.toml")
	codexCheck, err := checkCodexConfig(codexConfigPath, codexNotifyPath)
	if err != nil {
		return DoctorResult{}, err
	}
	add(codexCheck)

	return out, nil
}

func checkManagedScript(name, path string) DoctorCheck {
	info, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return DoctorCheck{Name: name, Status: "fail", Message: "file not found", Path: path}
		}
		return DoctorCheck{Name: name, Status: "fail", Message: fmt.Sprintf("stat error: %v", err), Path: path}
	}
	if info.Mode()&0o111 == 0 {
		return DoctorCheck{Name: name, Status: "fail", Message: "not executable", Path: path}
	}
	return DoctorCheck{Name: name, Status: "pass", Message: "installed", Path: path}
}

func checkClaudeSettings(path, hookEmitPath string) (DoctorCheck, error) {
	raw, err := readOptional(path)
	if err != nil {
		return DoctorCheck{}, err
	}
	if len(raw) == 0 {
		return DoctorCheck{Name: "claude_settings", Status: "fail", Message: "settings.json not found or empty", Path: path}, nil
	}

	var root map[string]any
	if err := json.Unmarshal(raw, &root); err != nil {
		return DoctorCheck{Name: "claude_settings", Status: "fail", Message: "invalid JSON", Path: path}, nil
	}
	hooks, _ := root["hooks"].(map[string]any)
	if hooks == nil {
		return DoctorCheck{Name: "claude_settings", Status: "fail", Message: "hooks object missing", Path: path}, nil
	}
	requiredEvents := []string{"Notification", "Stop", "SubagentStop"}
	for _, event := range requiredEvents {
		if !containsClaudeHookPath(hooks[event], hookEmitPath) {
			return DoctorCheck{
				Name:    "claude_settings",
				Status:  "fail",
				Message: fmt.Sprintf("missing hook command for %s", event),
				Path:    path,
			}, nil
		}
	}
	return DoctorCheck{Name: "claude_settings", Status: "pass", Message: "hooks configured", Path: path}, nil
}

func containsClaudeHookPath(raw any, hookEmitPath string) bool {
	entries, _ := raw.([]any)
	for _, entryAny := range entries {
		entry, _ := entryAny.(map[string]any)
		if entry == nil {
			continue
		}
		hookList, _ := entry["hooks"].([]any)
		for _, hookAny := range hookList {
			hook, _ := hookAny.(map[string]any)
			if hook == nil {
				continue
			}
			command, _ := hook["command"].(string)
			if strings.Contains(command, hookEmitPath) {
				return true
			}
		}
	}
	return false
}

func checkCodexConfig(path, codexNotifyPath string) (DoctorCheck, error) {
	raw, err := readOptional(path)
	if err != nil {
		return DoctorCheck{}, err
	}
	if len(raw) == 0 {
		return DoctorCheck{Name: "codex_config", Status: "fail", Message: "config.toml not found or empty", Path: path}, nil
	}
	text := string(raw)
	if !notifyLinePattern.MatchString(text) {
		return DoctorCheck{Name: "codex_config", Status: "fail", Message: "notify setting not found", Path: path}, nil
	}
	if strings.Contains(text, codexNotifyPath) {
		return DoctorCheck{Name: "codex_config", Status: "pass", Message: "notify points to agtmux-codex-notify", Path: path}, nil
	}
	return DoctorCheck{
		Name:    "codex_config",
		Status:  "warn",
		Message: "notify exists but does not reference agtmux-codex-notify",
		Path:    path,
	}, nil
}
