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

func TestAssignCodexHintsToCandidates(t *testing.T) {
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
	if got := byRuntime["rt-2"].label; got != "new prompt" {
		t.Fatalf("expected rt-2 to map newest hint, got %q", got)
	}
	if got := byRuntime["rt-1"].label; got != "old prompt" {
		t.Fatalf("expected rt-1 to map second hint, got %q", got)
	}
}

func TestAssignCodexHintsToCandidatesPrefersRecentActivityOverStartTime(t *testing.T) {
	now := time.Now().UTC()
	candidates := []codexPaneCandidate{
		{
			paneKey:    "t1|%newer-start",
			runtimeID:  "rt-newer-start",
			startedAt:  now.Add(-1 * time.Minute),
			activityAt: now.Add(-10 * time.Minute),
		},
		{
			paneKey:    "t1|%older-start",
			runtimeID:  "rt-older-start",
			startedAt:  now.Add(-30 * time.Minute),
			activityAt: now.Add(-30 * time.Second),
		},
	}
	hints := []codexThreadHint{
		{id: "thread-most-recent", label: "most recent thread", at: now},
		{id: "thread-older", label: "older thread", at: now.Add(-5 * time.Minute)},
	}

	byRuntime, _ := assignCodexHintsToCandidates(candidates, hints)
	if got := byRuntime["rt-older-start"].label; got != "most recent thread" {
		t.Fatalf("expected rt-older-start to receive most recent hint by activity, got %q", got)
	}
	if got := byRuntime["rt-newer-start"].label; got != "older thread" {
		t.Fatalf("expected rt-newer-start to receive second hint, got %q", got)
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

func TestDerivePaneLastInteractionAtManagedPollerFallbackUsesTmuxActivity(t *testing.T) {
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
	if got == nil {
		t.Fatalf("expected managed poller fallback timestamp")
	}
	if !got.Equal(lastActivity) {
		t.Fatalf("expected %s, got %s", lastActivity.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
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
