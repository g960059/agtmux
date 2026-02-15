package integration

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDoctorPassesAfterInstall(t *testing.T) {
	home := t.TempDir()
	if _, err := Install(InstallOptions{
		HomeDir:   home,
		AGTMUXBin: "/usr/local/bin/agtmux",
	}); err != nil {
		t.Fatalf("install: %v", err)
	}

	result, err := Doctor(DoctorOptions{HomeDir: home})
	if err != nil {
		t.Fatalf("doctor: %v", err)
	}
	if !result.OK {
		t.Fatalf("expected doctor ok=true, got %+v", result)
	}
}

func TestDoctorFailsWhenIntegrationFilesMissing(t *testing.T) {
	home := t.TempDir()
	result, err := Doctor(DoctorOptions{HomeDir: home})
	if err != nil {
		t.Fatalf("doctor: %v", err)
	}
	if result.OK {
		t.Fatalf("expected doctor ok=false for missing setup, got %+v", result)
	}
}

func TestDoctorWarnsForCustomCodexNotify(t *testing.T) {
	home := t.TempDir()
	codexDir := filepath.Join(home, ".codex")
	if err := os.MkdirAll(codexDir, 0o755); err != nil {
		t.Fatalf("mkdir codex dir: %v", err)
	}
	if err := os.WriteFile(filepath.Join(codexDir, "config.toml"), []byte(`notify = ["sh","-lc","echo custom"]`), 0o600); err != nil {
		t.Fatalf("write codex config: %v", err)
	}

	result, err := Doctor(DoctorOptions{HomeDir: home})
	if err != nil {
		t.Fatalf("doctor: %v", err)
	}
	var foundWarn bool
	for _, c := range result.Checks {
		if c.Name == "codex_config" && c.Status == "warn" {
			foundWarn = true
		}
	}
	if !foundWarn {
		t.Fatalf("expected codex_config warn, got %+v", result.Checks)
	}
}
