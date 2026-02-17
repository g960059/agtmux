package daemon

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/g960059/agtmux/internal/model"
)

func TestExtractClaudeResumeID(t *testing.T) {
	tests := []struct {
		name    string
		cmdline string
		want    string
	}{
		{
			name:    "long flag",
			cmdline: "claude --dangerously-skip-permissions --resume 764d927d-d3a9-4772-9dc7-63bebabd77a2",
			want:    "764d927d-d3a9-4772-9dc7-63bebabd77a2",
		},
		{
			name:    "long flag equals",
			cmdline: "/bin/sh ~/.local/share/agtmux/bin/agtmux-claude --resume=abc-123 --foo bar",
			want:    "abc-123",
		},
		{
			name:    "short flag",
			cmdline: "claude -r session-1",
			want:    "session-1",
		},
		{
			name:    "invalid value",
			cmdline: "claude --resume /tmp/test",
			want:    "",
		},
		{
			name:    "missing value",
			cmdline: "claude --resume",
			want:    "",
		},
		{
			name:    "no resume",
			cmdline: "claude --dangerously-skip-permissions",
			want:    "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := extractClaudeResumeID(tt.cmdline)
			if got != tt.want {
				t.Fatalf("extractClaudeResumeID(%q)=%q want=%q", tt.cmdline, got, tt.want)
			}
		})
	}
}

func TestParsePSPIDCommandOutput(t *testing.T) {
	out := parsePSPIDCommandOutput(`
  123 /bin/zsh
  456 claude --resume 764d927d-d3a9-4772-9dc7-63bebabd77a2
  abc invalid
  789
`)
	if got := out[123]; got != "/bin/zsh" {
		t.Fatalf("pid 123 command=%q", got)
	}
	if got := out[456]; got == "" {
		t.Fatalf("pid 456 should be parsed")
	}
	if _, ok := out[789]; ok {
		t.Fatalf("pid 789 should be skipped due to missing command")
	}
}

func TestResolveClaudeSessionHintPrefersJSONLPreview(t *testing.T) {
	home := t.TempDir()
	workspace := "/Users/test/workspace"
	sessionID := "764d927d-d3a9-4772-9dc7-63bebabd77a2"
	projectDir := filepath.Join(home, ".claude", "projects", claudeProjectKey(workspace))
	if err := os.MkdirAll(projectDir, 0o755); err != nil {
		t.Fatalf("mkdir project dir: %v", err)
	}
	content := `{"type":"file-history-snapshot","messageId":"x"}
{"type":"user","message":{"role":"user","content":"Explain orchestration with gemini and codex for this pane"}}
`
	if err := os.WriteFile(filepath.Join(projectDir, sessionID+".jsonl"), []byte(content), 0o600); err != nil {
		t.Fatalf("write jsonl: %v", err)
	}

	hint := resolveClaudeSessionHint(home, workspace, sessionID, model.TargetKindLocal)
	if hint.label == "" {
		t.Fatalf("expected non-empty label")
	}
	if hint.source != "claude_session_jsonl" {
		t.Fatalf("expected claude_session_jsonl source, got %q", hint.source)
	}
	if hint.label != "Explain orchestration with gemini and codex for this pane" {
		t.Fatalf("unexpected label: %q", hint.label)
	}
	if hint.at.IsZero() {
		t.Fatalf("expected non-zero timestamp")
	}
}

func TestResolveClaudeSessionHintFallbackToResumeID(t *testing.T) {
	sessionID := "764d927d-d3a9-4772-9dc7-63bebabd77a2"
	hint := resolveClaudeSessionHint("", "", sessionID, model.TargetKindSSH)
	if hint.label != "claude 764d927d" {
		t.Fatalf("expected fallback label, got %q", hint.label)
	}
	if hint.source != "claude_resume_id" {
		t.Fatalf("expected claude_resume_id source, got %q", hint.source)
	}
	if !hint.at.IsZero() {
		t.Fatalf("fallback hint should not carry timestamp")
	}
}
