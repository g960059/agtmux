package daemon

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

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

func TestExtractClaudeSessionIDFromLsofOutput(t *testing.T) {
	raw := `
p2101
f4
n/Users/virtualmachine/.claude/projects/-Users-virtualmachine-ghq-github.com-g960059-agtmux/764d927d-d3a9-4772-9dc7-63bebabd77a2.jsonl
`
	got := extractClaudeSessionIDFromLsofOutput(raw)
	if got != "764d927d-d3a9-4772-9dc7-63bebabd77a2" {
		t.Fatalf("expected session id from lsof output, got %q", got)
	}
}

func TestBuildClaudeWorkspaceSessionHintsFallsBackToHistoryDisplay(t *testing.T) {
	home := t.TempDir()
	workspace := filepath.Join(home, "repo")
	if err := os.MkdirAll(workspace, 0o755); err != nil {
		t.Fatalf("mkdir workspace: %v", err)
	}
	claudeDir := filepath.Join(home, ".claude")
	if err := os.MkdirAll(claudeDir, 0o755); err != nil {
		t.Fatalf("mkdir claude dir: %v", err)
	}

	sessionID := "764d927d-d3a9-4772-9dc7-63bebabd77a2"
	timestamp := time.Now().UTC().Add(-2 * time.Minute).UnixMilli()
	historyLine := fmt.Sprintf(
		`{"sessionId":"%s","project":"%s","display":"Investigate pane lifecycle regressions","timestamp":%d}`+"\n",
		sessionID,
		workspace,
		timestamp,
	)
	if err := os.WriteFile(filepath.Join(claudeDir, "history.jsonl"), []byte(historyLine), 0o644); err != nil {
		t.Fatalf("write history: %v", err)
	}

	records := readClaudeHistoryRecords(home)
	hints := buildClaudeWorkspaceSessionHints(home, workspace, records)
	if len(hints) == 0 {
		t.Fatalf("expected at least one hint")
	}
	if hints[0].sessionID != sessionID {
		t.Fatalf("unexpected session id: %+v", hints[0])
	}
	if hints[0].hint.source != "claude_history_display" {
		t.Fatalf("unexpected hint source: %+v", hints[0])
	}
	if !strings.Contains(strings.ToLower(hints[0].hint.label), "investigate pane") {
		t.Fatalf("unexpected hint label: %+v", hints[0])
	}
}

func TestBuildClaudeWorkspaceSessionHintsPrefersJSONLOverHistoryDisplay(t *testing.T) {
	home := t.TempDir()
	workspace := filepath.Join(home, "repo")
	if err := os.MkdirAll(workspace, 0o755); err != nil {
		t.Fatalf("mkdir workspace: %v", err)
	}
	projectDir := filepath.Join(home, ".claude", "projects", claudeProjectKey(workspace))
	if err := os.MkdirAll(projectDir, 0o755); err != nil {
		t.Fatalf("mkdir project dir: %v", err)
	}

	sessionID := "764d927d-d3a9-4772-9dc7-63bebabd77a2"
	sessionPath := filepath.Join(projectDir, sessionID+".jsonl")
	sessionContent := `{"type":"user","message":{"content":[{"type":"text","text":"Implement robust claude label inference for tmux panes"}]}}` + "\n"
	if err := os.WriteFile(sessionPath, []byte(sessionContent), 0o644); err != nil {
		t.Fatalf("write session: %v", err)
	}
	modAt := time.Now().UTC().Add(-30 * time.Second)
	if err := os.Chtimes(sessionPath, modAt, modAt); err != nil {
		t.Fatalf("chtimes session: %v", err)
	}

	historyLine := fmt.Sprintf(
		`{"sessionId":"%s","project":"%s","display":"fallback display","timestamp":%d}`+"\n",
		sessionID,
		workspace,
		time.Now().UTC().Add(-1*time.Minute).UnixMilli(),
	)
	if err := os.WriteFile(filepath.Join(home, ".claude", "history.jsonl"), []byte(historyLine), 0o644); err != nil {
		t.Fatalf("write history: %v", err)
	}

	records := readClaudeHistoryRecords(home)
	hints := buildClaudeWorkspaceSessionHints(home, workspace, records)
	if len(hints) == 0 {
		t.Fatalf("expected at least one hint")
	}
	if hints[0].hint.source != "claude_session_jsonl" {
		t.Fatalf("expected claude_session_jsonl source, got %+v", hints[0])
	}
	if hints[0].hint.label != "Implement robust claude label inference for tmux panes" {
		t.Fatalf("unexpected label: %+v", hints[0])
	}
}

func TestAssignClaudeWorkspaceHintsToProbesUsesTemporalAffinity(t *testing.T) {
	now := time.Now().UTC()
	probes := []claudeRuntimeProbe{
		{runtimeID: "rt-new", startedAt: now.Add(-2 * time.Minute)},
		{runtimeID: "rt-old", startedAt: now.Add(-2 * time.Hour)},
	}
	sessionHints := []claudeWorkspaceSessionHint{
		{
			sessionID: "sid-old",
			hint: claudeSessionHint{
				label:  "Older thread",
				source: "claude_history_display",
				at:     now.Add(-3 * time.Hour),
			},
		},
		{
			sessionID: "sid-new",
			hint: claudeSessionHint{
				label:  "Latest thread",
				source: "claude_history_display",
				at:     now.Add(-90 * time.Second),
			},
		},
	}

	assigned := assignClaudeWorkspaceHintsToProbes(probes, sessionHints, nil)
	if got := assigned["rt-new"].label; got != "Latest thread" {
		t.Fatalf("rt-new hint=%q", got)
	}
	if got := assigned["rt-old"].label; got != "Older thread" {
		t.Fatalf("rt-old hint=%q", got)
	}
}

func TestGetClaudeHistoryRecordsUsesMtimeCache(t *testing.T) {
	home := t.TempDir()
	workspace := filepath.Join(home, "repo")
	if err := os.MkdirAll(workspace, 0o755); err != nil {
		t.Fatalf("mkdir workspace: %v", err)
	}
	claudeDir := filepath.Join(home, ".claude")
	if err := os.MkdirAll(claudeDir, 0o755); err != nil {
		t.Fatalf("mkdir claude dir: %v", err)
	}
	historyPath := filepath.Join(claudeDir, "history.jsonl")
	sessionID := "session-1"
	writeHistory := func(display string, ts int64) {
		line := fmt.Sprintf(
			`{"sessionId":"%s","project":"%s","display":"%s","timestamp":%d}`+"\n",
			sessionID,
			workspace,
			display,
			ts,
		)
		if err := os.WriteFile(historyPath, []byte(line), 0o644); err != nil {
			t.Fatalf("write history: %v", err)
		}
	}
	writeHistory("first display", time.Now().UTC().Add(-2*time.Minute).UnixMilli())
	info, err := os.Stat(historyPath)
	if err != nil {
		t.Fatalf("stat history: %v", err)
	}
	originalMod := info.ModTime().UTC()

	s := &Server{claudeHistoryTTL: time.Hour}
	records := s.getClaudeHistoryRecords(home)
	if len(records) != 1 || records[0].display != "first display" {
		t.Fatalf("unexpected records: %+v", records)
	}

	writeHistory("second display", time.Now().UTC().Add(-1*time.Minute).UnixMilli())
	if err := os.Chtimes(historyPath, originalMod, originalMod); err != nil {
		t.Fatalf("chtimes preserve modtime: %v", err)
	}
	cached := s.getClaudeHistoryRecords(home)
	if len(cached) != 1 || cached[0].display != "first display" {
		t.Fatalf("expected cached first display, got %+v", cached)
	}

	updatedMod := originalMod.Add(2 * time.Second)
	if err := os.Chtimes(historyPath, updatedMod, updatedMod); err != nil {
		t.Fatalf("chtimes updated modtime: %v", err)
	}
	reloaded := s.getClaudeHistoryRecords(home)
	if len(reloaded) != 1 || reloaded[0].display != "second display" {
		t.Fatalf("expected refreshed second display, got %+v", reloaded)
	}
}

func TestResolveClaudeSessionHintCachedUsesSessionPreviewCache(t *testing.T) {
	home := t.TempDir()
	workspace := filepath.Join(home, "repo")
	if err := os.MkdirAll(workspace, 0o755); err != nil {
		t.Fatalf("mkdir workspace: %v", err)
	}
	projectDir := filepath.Join(home, ".claude", "projects", claudeProjectKey(workspace))
	if err := os.MkdirAll(projectDir, 0o755); err != nil {
		t.Fatalf("mkdir project dir: %v", err)
	}
	sessionID := "764d927d-d3a9-4772-9dc7-63bebabd77a2"
	sessionPath := filepath.Join(projectDir, sessionID+".jsonl")
	writeSession := func(prompt string) {
		content := fmt.Sprintf(`{"type":"user","message":{"content":[{"type":"text","text":"%s"}]}}`+"\n", prompt)
		if err := os.WriteFile(sessionPath, []byte(content), 0o644); err != nil {
			t.Fatalf("write session: %v", err)
		}
	}
	writeSession("first prompt")
	info, err := os.Stat(sessionPath)
	if err != nil {
		t.Fatalf("stat session: %v", err)
	}
	originalMod := info.ModTime().UTC()

	s := &Server{
		claudePreview:    map[string]claudePreviewCacheEntry{},
		claudePreviewTTL: time.Hour,
	}
	hint := s.resolveClaudeSessionHintCached(home, workspace, sessionID, model.TargetKindLocal)
	if hint.label != "first prompt" || hint.source != "claude_session_jsonl" {
		t.Fatalf("unexpected first hint: %+v", hint)
	}

	writeSession("second prompt")
	if err := os.Chtimes(sessionPath, originalMod, originalMod); err != nil {
		t.Fatalf("chtimes preserve modtime: %v", err)
	}
	cachedHint := s.resolveClaudeSessionHintCached(home, workspace, sessionID, model.TargetKindLocal)
	if cachedHint.label != "first prompt" {
		t.Fatalf("expected cached first prompt, got %+v", cachedHint)
	}

	updatedMod := originalMod.Add(2 * time.Second)
	if err := os.Chtimes(sessionPath, updatedMod, updatedMod); err != nil {
		t.Fatalf("chtimes updated modtime: %v", err)
	}
	refreshed := s.resolveClaudeSessionHintCached(home, workspace, sessionID, model.TargetKindLocal)
	if refreshed.label != "second prompt" {
		t.Fatalf("expected refreshed second prompt, got %+v", refreshed)
	}
}
