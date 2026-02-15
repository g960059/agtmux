package integration

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestInstallCreatesManagedFiles(t *testing.T) {
	home := t.TempDir()

	res, err := Install(InstallOptions{
		HomeDir:   home,
		AGTMUXBin: "/usr/local/bin/agtmux",
	})
	if err != nil {
		t.Fatalf("install: %v", err)
	}
	if res.DryRun {
		t.Fatalf("expected non dry-run result")
	}

	binDir := filepath.Join(home, ".local", "share", "agtmux", "bin")
	mustFiles := []string{
		filepath.Join(binDir, "agtmux-codex"),
		filepath.Join(binDir, "agtmux-claude"),
		filepath.Join(binDir, "agtmux-hook-emit"),
		filepath.Join(binDir, "agtmux-codex-notify"),
		filepath.Join(home, ".claude", "settings.json"),
		filepath.Join(home, ".codex", "config.toml"),
	}
	for _, path := range mustFiles {
		if _, err := os.Stat(path); err != nil {
			t.Fatalf("expected managed file %s: %v", path, err)
		}
	}

	claudeRaw, err := os.ReadFile(filepath.Join(home, ".claude", "settings.json"))
	if err != nil {
		t.Fatalf("read claude settings: %v", err)
	}
	claudeText := string(claudeRaw)
	if !strings.Contains(claudeText, "Notification") || !strings.Contains(claudeText, "Stop") {
		t.Fatalf("claude hooks should contain Notification/Stop: %s", claudeText)
	}

	codexRaw, err := os.ReadFile(filepath.Join(home, ".codex", "config.toml"))
	if err != nil {
		t.Fatalf("read codex config: %v", err)
	}
	if !strings.Contains(string(codexRaw), "notify = [") {
		t.Fatalf("codex config should contain notify setting: %s", string(codexRaw))
	}

	// Idempotency: running twice should not fail or duplicate managed blocks.
	if _, err := Install(InstallOptions{
		HomeDir:   home,
		AGTMUXBin: "/usr/local/bin/agtmux",
	}); err != nil {
		t.Fatalf("second install: %v", err)
	}
}

func TestInstallSkipsCodexNotifyWhenExistingAndNoForce(t *testing.T) {
	home := t.TempDir()
	codexDir := filepath.Join(home, ".codex")
	if err := os.MkdirAll(codexDir, 0o755); err != nil {
		t.Fatalf("mkdir codex dir: %v", err)
	}
	configPath := filepath.Join(codexDir, "config.toml")
	original := "notify = [\"existing-notifier\"]\n"
	if err := os.WriteFile(configPath, []byte(original), 0o600); err != nil {
		t.Fatalf("write codex config: %v", err)
	}

	res, err := Install(InstallOptions{
		HomeDir:   home,
		AGTMUXBin: "/usr/local/bin/agtmux",
	})
	if err != nil {
		t.Fatalf("install: %v", err)
	}
	if len(res.Warnings) == 0 {
		t.Fatalf("expected warning when existing notify is kept")
	}
	after, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatalf("read codex config after install: %v", err)
	}
	if string(after) != original {
		t.Fatalf("expected codex config unchanged, got: %s", string(after))
	}
}

func TestInstallDryRunDoesNotWriteFiles(t *testing.T) {
	home := t.TempDir()
	res, err := Install(InstallOptions{
		HomeDir:   home,
		AGTMUXBin: "/usr/local/bin/agtmux",
		DryRun:    true,
	})
	if err != nil {
		t.Fatalf("dry-run install: %v", err)
	}
	if !res.DryRun {
		t.Fatalf("expected dry-run result")
	}
	if _, err := os.Stat(filepath.Join(home, ".claude", "settings.json")); !os.IsNotExist(err) {
		t.Fatalf("dry-run should not write claude settings, err=%v", err)
	}
}

func TestInstallExplicitAllSkipsWritesNothing(t *testing.T) {
	home := t.TempDir()
	res, err := Install(InstallOptions{
		HomeDir:          home,
		AGTMUXBin:        "/usr/local/bin/agtmux",
		TogglesExplicit:  true,
		InstallClaude:    false,
		InstallCodex:     false,
		InstallWrappers:  false,
		ForceCodexNotify: false,
	})
	if err != nil {
		t.Fatalf("install with explicit skips: %v", err)
	}
	if len(res.FilesWritten) != 0 {
		t.Fatalf("expected no writes, got %+v", res.FilesWritten)
	}
	if _, err := os.Stat(filepath.Join(home, ".claude", "settings.json")); !os.IsNotExist(err) {
		t.Fatalf("expected no claude settings write, err=%v", err)
	}
	if _, err := os.Stat(filepath.Join(home, ".codex", "config.toml")); !os.IsNotExist(err) {
		t.Fatalf("expected no codex config write, err=%v", err)
	}
}

func TestInstallForceCodexNotifyReplacesMultilineSafely(t *testing.T) {
	home := t.TempDir()
	codexDir := filepath.Join(home, ".codex")
	if err := os.MkdirAll(codexDir, 0o755); err != nil {
		t.Fatalf("mkdir codex dir: %v", err)
	}
	configPath := filepath.Join(codexDir, "config.toml")
	original := "notify = [\n  \"sh\",\n  \"-lc\",\n  \"echo hi\"\n]\n"
	if err := os.WriteFile(configPath, []byte(original), 0o600); err != nil {
		t.Fatalf("write codex config: %v", err)
	}

	_, err := Install(InstallOptions{
		HomeDir:          home,
		AGTMUXBin:        "/usr/local/bin/agtmux",
		TogglesExplicit:  true,
		InstallClaude:    false,
		InstallCodex:     true,
		InstallWrappers:  false,
		ForceCodexNotify: true,
	})
	if err != nil {
		t.Fatalf("force notify install: %v", err)
	}

	after, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatalf("read codex config after force replace: %v", err)
	}
	text := string(after)
	if !strings.Contains(text, `$1`) {
		t.Fatalf("expected codex notify arg placeholder preserved, got: %s", text)
	}
	if strings.Contains(text, `echo hi`) {
		t.Fatalf("expected old multiline notify body removed, got: %s", text)
	}
}

func TestRenderCodexNotifyScriptClassifiesPayloadEventType(t *testing.T) {
	script := renderCodexNotifyScript("/usr/local/bin/agtmux")
	required := []string{
		`EVENT_TYPE="agent-turn-complete"`,
		`approval-requested`,
		`input-requested`,
		`runtime-error`,
		`PAYLOAD_LOWER=`,
	}
	for _, needle := range required {
		if !strings.Contains(script, needle) {
			t.Fatalf("rendered script must contain %q, got: %s", needle, script)
		}
	}
	if strings.Contains(script, `*failed*|*error*`) {
		t.Fatalf("script must not use broad failed/error wildcard match: %s", script)
	}
}
