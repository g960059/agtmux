package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestEventEmitCallsAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/events", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode request: %v", err)
		}
		if req["target"] != "local" || req["pane_id"] != "%1" || req["source"] != "notify" || req["event_type"] != "agent-turn-complete" {
			t.Fatalf("unexpected event request: %+v", req)
		}
		if dedupe, _ := req["dedupe_key"].(string); strings.TrimSpace(dedupe) == "" {
			t.Fatalf("dedupe_key should be auto-populated: %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-15T00:00:00Z","event_id":"ev-1","status":"pending_bind"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{
		"event", "emit",
		"--target", "local",
		"--pane", "%1",
		"--agent", "codex",
		"--source", "notify",
		"--type", "agent-turn-complete",
		"--json",
	})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"status":"pending_bind"`) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestIntegrationInstallDryRunJSON(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://unused", &http.Client{}, out, errOut)
	home := t.TempDir()

	code := r.Run(context.Background(), []string{
		"integration", "install",
		"--home", home,
		"--dry-run",
		"--json",
	})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"dry_run":true`) {
		t.Fatalf("expected dry_run=true in output, got %s", out.String())
	}

	if _, err := os.Stat(filepath.Join(home, ".claude", "settings.json")); !os.IsNotExist(err) {
		t.Fatalf("dry-run should not write claude settings, err=%v", err)
	}
}

func TestIntegrationDoctorJSONFailsWhenSetupMissing(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://unused", &http.Client{}, out, errOut)
	home := t.TempDir()

	code := r.Run(context.Background(), []string{
		"integration", "doctor",
		"--home", home,
		"--json",
	})
	if code != 1 {
		t.Fatalf("expected exit 1 for missing setup, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"ok":false`) {
		t.Fatalf("expected ok=false in output, got %s", out.String())
	}
}

func TestIntegrationDoctorJSONPassesAfterInstall(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://unused", &http.Client{}, out, errOut)
	home := t.TempDir()

	installCode := r.Run(context.Background(), []string{
		"integration", "install",
		"--home", home,
		"--json",
	})
	if installCode != 0 {
		t.Fatalf("expected install exit 0, got %d stderr=%s", installCode, errOut.String())
	}

	out.Reset()
	errOut.Reset()
	doctorCode := r.Run(context.Background(), []string{
		"integration", "doctor",
		"--home", home,
		"--json",
	})
	if doctorCode != 0 {
		t.Fatalf("expected doctor exit 0, got %d stderr=%s", doctorCode, errOut.String())
	}
	if !strings.Contains(out.String(), `"ok":true`) {
		t.Fatalf("expected ok=true in doctor output, got %s", out.String())
	}
}

func TestEventEmitRejectsPaneAndRuntimeTogether(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://unused", &http.Client{}, out, errOut)

	code := r.Run(context.Background(), []string{
		"event", "emit",
		"--target", "local",
		"--pane", "%1",
		"--runtime", "rt-1",
		"--source", "notify",
		"--type", "agent-turn-complete",
	})
	if code != 2 {
		t.Fatalf("expected usage exit 2, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "usage: agtmux event emit") {
		t.Fatalf("expected usage message, got %s", errOut.String())
	}
}
