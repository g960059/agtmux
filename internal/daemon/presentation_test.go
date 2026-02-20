package daemon

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/model"
)

func TestDerivePaneSessionLabelPrefersCodexThreadHint(t *testing.T) {
	pane := model.Pane{
		PaneID:      "%1",
		SessionName: "exp/go-codex-implementation-poc",
	}
	label, source := derivePaneSessionLabel(
		"managed",
		pane,
		"rt-1",
		"t1|%1",
		map[string]actionInputHint{
			"rt-1": {preview: "fallback first input"},
		},
		nil,
		nil,
		"codex",
		codexThreadHint{label: "Fix status grouping in AGTMUX UI"},
		true,
	)
	if label != "Fix status grouping in AGTMUX UI" {
		t.Fatalf("expected codex thread label, got %q", label)
	}
	if source != "codex_thread_list" {
		t.Fatalf("expected codex_thread_list source, got %q", source)
	}
}

func TestShouldUseCodexWorkspaceHint(t *testing.T) {
	path := "/Users/virtualmachine/worktree"
	if !shouldUseCodexWorkspaceHint(path, map[string]int{normalizeCodexWorkspacePath(path): 1}) {
		t.Fatalf("expected hint to be enabled for single codex pane per workspace")
	}
	if shouldUseCodexWorkspaceHint(path, map[string]int{normalizeCodexWorkspacePath(path): 2}) {
		t.Fatalf("expected hint to be disabled for multi codex panes per workspace")
	}
	if shouldUseCodexWorkspaceHint("", map[string]int{}) {
		t.Fatalf("expected empty workspace path to disable hint")
	}
}

func TestAssignCodexHintsToCandidatesRequiresDeterministicBinding(t *testing.T) {
	now := time.Now().UTC()
	candidates := []codexPaneCandidate{
		{paneKey: "t1|%2", runtimeID: "rt-2", startedAt: now.Add(-1 * time.Minute), activityAt: now.Add(-1 * time.Minute)},
		{paneKey: "t1|%1", runtimeID: "rt-1", startedAt: now.Add(-3 * time.Minute), activityAt: now.Add(-3 * time.Minute)},
	}
	hints := []codexThreadHint{
		{id: "thread-new", label: "new prompt", at: now},
		{id: "thread-old", label: "old prompt", at: now.Add(-2 * time.Minute)},
	}

	byRuntime, byPane := assignCodexHintsToCandidates(candidates, hints)
	if len(byPane) != 0 {
		t.Fatalf("expected no pane fallback assignments, got %+v", byPane)
	}
	if len(byRuntime) != 0 {
		t.Fatalf("expected no runtime mapping without deterministic id match, got %+v", byRuntime)
	}
}

func TestAssignCodexHintsToCandidatesSingleCandidateSingleHintFallback(t *testing.T) {
	now := time.Now().UTC()
	candidates := []codexPaneCandidate{
		{
			paneKey:    "t1|%single",
			runtimeID:  "rt-single",
			startedAt:  now.Add(-2 * time.Minute),
			activityAt: now.Add(-30 * time.Second),
		},
	}
	hints := []codexThreadHint{
		{id: "thread-active", label: "single active thread", at: now},
	}

	byRuntime, _ := assignCodexHintsToCandidates(candidates, hints)
	if got := byRuntime["rt-single"].label; got != "single active thread" {
		t.Fatalf("expected deterministic single fallback mapping, got %q", got)
	}
}

func TestAssignCodexHintsToCandidatesPrefersThreadIDBinding(t *testing.T) {
	now := time.Now().UTC()
	candidates := []codexPaneCandidate{
		{
			paneKey:    "t1|%running",
			runtimeID:  "rt-running",
			threadID:   "019c57c0-9429-73d3-8a96-53bfc6e80d7f",
			labelHint:  "すでに残っている残留codexはありません。",
			startedAt:  now.Add(-2 * time.Hour),
			activityAt: now.Add(-2 * time.Second),
		},
		{
			paneKey:    "t1|%idle",
			runtimeID:  "rt-idle",
			threadID:   "019c61bc-1144-7700-826a-b2eeeb910d0c",
			labelHint:  "/tmp/agtmux_pr_review_uiux_20260215.md",
			startedAt:  now.Add(-30 * time.Minute),
			activityAt: now.Add(-10 * time.Hour),
		},
	}
	hints := []codexThreadHint{
		{
			id:    "019c61bc-1144-7700-826a-b2eeeb910d0c",
			label: "/tmp/agtmux_pr_review_uiux_20260215.md",
			at:    now,
		},
		{
			id:    "019c57c0-9429-73d3-8a96-53bfc6e80d7f",
			label: "すでに残っている残留codexはありません。",
			at:    now.Add(-1 * time.Minute),
		},
	}
	byRuntime, _ := assignCodexHintsToCandidates(candidates, hints)
	if got := byRuntime["rt-running"].id; got != "019c57c0-9429-73d3-8a96-53bfc6e80d7f" {
		t.Fatalf("expected rt-running to keep thread-id-matched hint, got %q", got)
	}
	if got := byRuntime["rt-idle"].id; got != "019c61bc-1144-7700-826a-b2eeeb910d0c" {
		t.Fatalf("expected rt-idle to keep thread-id-matched hint, got %q", got)
	}
}

func TestExtractCodexThreadIDFromLsofOutput(t *testing.T) {
	raw := `
p10304
fcwd
n/Users/virtualmachine/ghq/github.com/g960059/agtmux/.worktrees/exp/go-codex-implementation-poc
f16
n/Users/virtualmachine/.codex/sessions/2026/02/13/rollout-2026-02-13T08-06-04-019c57c0-9429-73d3-8a96-53bfc6e80d7f.jsonl
`
	got := extractCodexThreadIDFromLsofOutput(raw)
	if got != "019c57c0-9429-73d3-8a96-53bfc6e80d7f" {
		t.Fatalf("expected thread id from lsof output, got %q", got)
	}
}

func TestExtractCodexThreadIDFromCommand(t *testing.T) {
	tests := []struct {
		name    string
		command string
		want    string
	}{
		{
			name:    "resume subcommand",
			command: "codex resume 019c57c0-9429-73d3-8a96-53bfc6e80d7f",
			want:    "019c57c0-9429-73d3-8a96-53bfc6e80d7f",
		},
		{
			name:    "resume long flag",
			command: "node /Users/vm/.nvm/versions/node/v24.12.0/bin/codex --resume=019c61bc-1144-7700-826a-b2eeeb910d0c --yolo",
			want:    "019c61bc-1144-7700-826a-b2eeeb910d0c",
		},
		{
			name:    "no session id",
			command: "codex --dangerously-bypass-approvals",
			want:    "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := extractCodexThreadIDFromCommand(tt.command); got != tt.want {
				t.Fatalf("extractCodexThreadIDFromCommand(%q)=%q want=%q", tt.command, got, tt.want)
			}
		})
	}
}

func TestDerivePaneLastInteractionAtManagedPollerDoesNotUseTmuxActivityFallback(t *testing.T) {
	now := time.Now().UTC()
	lastActivity := now.Add(-10 * time.Second)
	updatedAt := now.Add(-2 * time.Second)
	got := derivePaneLastInteractionAt(
		"managed",
		"",
		"t1|%1",
		nil,
		nil,
		nil,
		"codex",
		codexThreadHint{},
		false,
		string(model.SourcePoller),
		"",
		nil,
		&lastActivity,
		updatedAt,
	)
	if got != nil {
		t.Fatalf("expected nil for managed poller fallback, got %s", got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtUnmanagedFallsBackToLastActivity(t *testing.T) {
	now := time.Now().UTC()
	lastActivity := now.Add(-45 * time.Second)
	updatedAt := now.Add(-30 * time.Second)
	got := derivePaneLastInteractionAt(
		"none",
		"",
		"t1|%2",
		nil,
		nil,
		nil,
		"none",
		codexThreadHint{},
		false,
		string(model.SourcePoller),
		"",
		nil,
		&lastActivity,
		updatedAt,
	)
	if got == nil {
		t.Fatalf("expected last interaction timestamp")
	}
	if !got.Equal(lastActivity) {
		t.Fatalf("expected %s, got %s", lastActivity.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtUsesCodexHintTimestamp(t *testing.T) {
	now := time.Now().UTC()
	hintTime := now.Add(-5 * time.Minute)
	got := derivePaneLastInteractionAt(
		"managed",
		"",
		"t1|%3",
		nil,
		nil,
		nil,
		"codex",
		codexThreadHint{
			label: "Investigate daemon reconnect behavior",
			at:    hintTime,
		},
		true,
		string(model.SourcePoller),
		"",
		nil,
		nil,
		now,
	)
	if got == nil {
		t.Fatalf("expected codex hint timestamp")
	}
	if !got.Equal(hintTime) {
		t.Fatalf("expected %s, got %s", hintTime.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtIgnoresAdministrativeRuntimeEvent(t *testing.T) {
	now := time.Now().UTC()
	userInputAt := now.Add(-4 * time.Minute)
	got := derivePaneLastInteractionAt(
		"managed",
		"rt-1",
		"t1|%4",
		map[string]actionInputHint{
			"t1|%4": {preview: "user prompt", at: userInputAt},
		},
		nil,
		map[string]runtimeEventHint{
			"rt-1": {at: now, event: "action.view-output"},
		},
		"claude",
		codexThreadHint{},
		false,
		string(model.SourcePoller),
		"",
		nil,
		nil,
		now,
	)
	if got == nil {
		t.Fatalf("expected fallback to user input timestamp")
	}
	if !got.Equal(userInputAt) {
		t.Fatalf("expected %s, got %s", userInputAt.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtIgnoresAdministrativeStateEvent(t *testing.T) {
	now := time.Now().UTC()
	stateEventAt := now.Add(-1 * time.Second)
	got := derivePaneLastInteractionAt(
		"managed",
		"",
		"t1|%5",
		nil,
		nil,
		nil,
		"claude",
		codexThreadHint{},
		false,
		string(model.SourceWrapper),
		"action.view-output",
		&stateEventAt,
		nil,
		now,
	)
	if got != nil {
		t.Fatalf("expected nil for managed pane when only administrative state event exists, got %s", got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneSessionActiveTimeManagedCodexWithoutThreadHintReturnsUnknown(t *testing.T) {
	now := time.Now().UTC()
	lastInput := now.Add(-2 * time.Minute)
	sessionTime := derivePaneSessionActiveTime(
		"managed",
		"rt-1",
		"t1|%1",
		nil,
		map[string]actionInputHint{
			"rt-1": {preview: "fallback", at: lastInput},
		},
		nil,
		"codex",
		codexThreadHint{},
		false,
		claudeSessionHint{},
		false,
		string(model.SourceNotify),
		"agent-turn-complete",
		nil,
		nil,
		now,
	)
	if sessionTime.at != nil {
		t.Fatalf("expected unknown session time for codex without thread hint, got %+v", sessionTime)
	}
}

func TestDerivePaneSessionActiveTimeManagedClaudeWithoutSessionHintReturnsUnknown(t *testing.T) {
	now := time.Now().UTC()
	lastInput := now.Add(-2 * time.Minute)
	sessionTime := derivePaneSessionActiveTime(
		"managed",
		"rt-2",
		"t1|%2",
		nil,
		map[string]actionInputHint{
			"rt-2": {preview: "fallback", at: lastInput},
		},
		nil,
		"claude",
		codexThreadHint{},
		false,
		claudeSessionHint{},
		false,
		string(model.SourceHook),
		"task-finished",
		nil,
		nil,
		now,
	)
	if sessionTime.at != nil {
		t.Fatalf("expected unknown session time for claude without resolved session hint, got %+v", sessionTime)
	}
}
